# Plano — pipeline CPU/GPU: um decode, dois artefatos

> **Status: proposto.** Nada do pipeline entra no código antes do gate da
> [Fase 0](#5-fase-0--medição-o-gate). O que existe hoje é a instrumentação.
>
> Complementa [`vision-classification-speed.md`](./vision-classification-speed.md)
> (que otimizou o *request*, não a duplicação descrita aqui) e
> [`thumbnail-cache-and-dhash-dedupe.md`](./thumbnail-cache-and-dhash-dedupe.md)
> (que criou o comando de reparo usado como base do produtor).

## 0. Objetivo

Hoje o scan faz, para cada foto: **um decode completo para a thumbnail**, e
depois **outro decode completo para a classificação**. Também lê o arquivo duas
vezes do SMB. A fase de thumbnails é mono-thread, então a GPU fica ociosa
durante ela; a fase de classificação é sequencial, então o CPU fica ocioso
durante ela.

O objetivo é: **um único read + um único decode por arquivo**, produzindo dois
artefatos (thumbnail 300 px e JPEG 1024 px para a visão), com os produtores de
CPU alimentando um consumidor de GPU. O ganho precisa vir da **redução de
trabalho**, não da sobreposição — ver §2.

## 1. O que o código faz hoje (verificado)

| Trabalho | Onde | Observação |
|---|---|---|
| Thumbnail de foto: `image::open` → dHash → downscale 300 → JPEG q80 | [`thumbnail.rs:164`](../sherlock/desktop/src-tauri/src/thumbnail.rs:164) | decode completo |
| Visão de foto: `prepare` → `image::open` de novo → resize 1024 → JPEG q88 | [`vision_prep.rs:112`](../sherlock/desktop/src-tauri/src/vision_prep.rs:112) | **segundo decode completo + segundo read no SMB** |
| Thumbnail de vídeo: 1 keyframe (`extract_keyframes(.., 1)`) | [`thumbnail.rs:355`](../sherlock/desktop/src-tauri/src/thumbnail.rs:355) | ffmpeg seek+decode |
| Visão de vídeo: 5 keyframes + metadata + legendas | [`classify.rs:987`](../sherlock/desktop/src-tauri/src/classify.rs:987) | ffmpeg seek+decode de novo |
| Thumbnail de PDF: 2 páginas @96 dpi + `find_content_pages` | [`thumbnail.rs:253`](../sherlock/desktop/src-tauri/src/thumbnail.rs:253) | pdfium |
| Visão de PDF: `find_content_pages` de novo + 1 página @200 dpi | [`classify.rs:1193`](../sherlock/desktop/src-tauri/src/classify.rs:1193) | pdfium de novo |
| Fase 2 do scan: **mono-thread** | [`scan.rs:445`](../sherlock/desktop/src-tauri/src/scan.rs:445) | `REPAIR_WORKERS` só é usado no reparo ([`scan.rs:1675`](../sherlock/desktop/src-tauri/src/scan.rs:1675)) |
| Fase 3 do scan: **sequencial**, uma requisição por vez | [`scan.rs:810`](../sherlock/desktop/src-tauri/src/scan.rs:810) | correto: Ollama serializa |
| `classify_only` não gera thumbnail | [`ScanMode::ClassifyOnly`](../sherlock/desktop/src-tauri/src/scan.rs:82) | CPU ociosa com a GPU ocupada |
| `repair_thumbnails` recusa rodar com scan ativo | [`lib.rs:481`](../sherlock/desktop/src-tauri/src/lib.rs:481) | impede a sobreposição manual hoje |

## 2. Por que a sobreposição sozinha não paga

O GPU só executa **prefill + decode**; o decode/resize/tokenização da imagem
acontece na CPU, dentro do Ollama (é o `gap` de
[`vision_estimate::Breakdown`](../sherlock/desktop/src-tauri/src/vision_estimate.rs:51)).

| Config medida | wall/img | GPU ativo | CPU+rede (`gap`) |
|---|---|---|---|
| 3b, 768 px, ctx 2048 | 2,09 s | 0,64 s (31 %) | 1,45 s (69 %) |
| 3b, 1024 px | 3,45 s | 1,11 s (32 %) | 2,34 s (68 %) |
| 7b, 1024 px | 7,80 s | 5,23 s (67 %) | 2,57 s (33 %) |

Com o 3b (modelo recomendado por padrão) o caminho crítico já é a CPU. Encher a
janela de GPU com trabalho de CPU não reduz o tempo total — e pode piorar, se
os workers roubarem CPU do pré-processador do Ollama. Logo:

**Medido depois** (174 arquivos, 1,41 MB, 3b — §5.2), a ordem se inverteu: o
duplicado vale 6 % e a sobreposição vale até 20 %. O ganho principal é o
**pipeline** (§4.3), que usa a janela de 1,5 s de CPU do Ollama para adiantar o
próximo arquivo; o decode compartilhado é o bônus.

Uma observação importante de método: as tabelas de
[`vision-classification-speed.md`](./vision-classification-speed.md) medem
apenas o *request* — `prepare` nunca entrou na conta. O custo duplicado que este
plano ataca é justamente o que aquelas tabelas não mostram.

## 3. Decisões

| Item | Decisão | Por quê |
|---|---|---|
| Artefatos | 300 px (thumb) **e** 1024 px (visão) do mesmo decode | abaixo de 768 px a qualidade cai (o 3b trocou *burro* por *elefante*) |
| Filtro do downscale em `prepare` | **implementado** (`image_filter`): `auto` = box para JPEG/HEIC, Lanczos3 para PNG/BMP/TIFF; `box` e `lanczos` também aceitos | medido: −187,6 ms/arquivo (−33 % do `prepare`, −6,2 % do total). Texto fino é o risco, e screenshot é onde ele aparece |
| Escopo da 1ª iteração | **somente imagens estáticas** | vídeo/PDF exigem unificar keyframes (1 vs 5) e DPI (96 vs 200) |
| Vídeo/PDF | caminho atual, intocados | risco alto, ganho menor, medição separada |
| Ordem no pipeline | produtores ordenados por `rel_path`, consumidor FIFO | mantém o cursor e o resume do job monotônicos |
| Dedupe de classificação | continua no consumidor (single-thread) | `DedupeIndex` não é `Sync`; o representante passa a depender da ordem de conclusão (aceitável) |
| Workers de CPU | default **2**, mais só em disco local | 4 workers já saturam o SMB medido (13,5 → 15 MB/s) |
| Gate | teto do pipeline < 20 % **ou** produtor ≥ consumidor ⇒ **descartar o pipeline** | requisito explícito do usuário: não pode ficar mais lento. O duplicado isolado não serve de critério: ele ignora a sobreposição, que é o objetivo |

## 4. Desenho

### 4.1 Um decode, dois artefatos

`thumbnail.rs` passa a expor o decode intermediário:

```rust
/// Decodifica, aplica orientação EXIF e calcula o dHash uma única vez.
pub struct DecodedSource {
    pub image: image::DynamicImage,
    pub dhash: u64,
}

pub fn decode_source(path: &Path) -> Option<DecodedSource>;

/// Variantes que aceitam a imagem já decodificada.
pub fn thumbnail_from_decoded(
    decoded: &DecodedSource,
    thumb_dir: &Path,
    rel_path: &str,
) -> Option<ThumbnailResult>;
```

`generate_thumbnail` vira um wrapper (`decode_source` + `thumbnail_from_decoded`)
para não mexer no reparo existente.

### 4.2 JPEG de visão sem reler a origem

```rust
/// Escreve o JPEG de `max_side` (q88) a partir de uma imagem já decodificada.
/// `prepare()` continua sendo o wrapper que lê do disco para os chamadores atuais.
pub fn prepare_from_decoded(
    source: &Path,
    decoded: &image::DynamicImage,
    max_side: u32,
    dest: &Path,
) -> AppResult<PreparedImage>;
```

Ciclo de vida do artefato (decisão na Fase 0, ver §5):

- `transitório` (default proposto): arquivo temporário no diretório do
  pipeline, apagado pelo consumidor. Disco limitado pela profundidade da fila.
- `persistente` (opcional): `cache/prepared/<root_key>/<rel_path>.jpg`, apagado
  quando a linha é classificada; entra em `purge_root` e `cleanup_deleted_caches`.
  Torna o resume de `classify_only` instantâneo, ao custo de ~200 KB × pendentes.
- Chave do cache: `image_max_side` + qualidade JPEG. Mudar a resolução no modal
  invalida tudo.

### 4.3 Pipeline produtor/consumidor

```
probes (ordenados por rel_path)
        │
        ├─ produtor 1 ─┐
        ├─ produtor 2 ─┴─► fila limitada (ex.: 8) ─► consumidor (GPU, único)
        │                    (row, dhash, prepared?)        │
        └─ leitura SMB, decode, dHash, thumb 300, JPEG 1024 │
                                                             ├─ dedupe por dhash
                                                             └─ classify + UPDATE
```

- Fila limitada: pressão de memória/disco nula; produtores bloqueiam quando o
  consumidor está atrasado (exatamente o comportamento desejado).
- Cancelamento: `AtomicBool` já usado; produtores param, a fila é drenada sem
  classificar o que sobrou, o job fica retomável pelo cursor.
- Progresso: dois contadores (`thumbnailed`, `classified`) — a UI atual mostra
  só um.

### 4.4 Backlog combinado

Seleção única em vez de dois passes:

```sql
SELECT ... FROM files
WHERE deleted_at IS NULL
  AND (thumb_path IS NULL OR confidence = 0)
ORDER BY rel_path
```

Assim os dois backlogs atuais (≈32 mil thumbs faltando e os não classificados)
viram **um** passe, com um read por arquivo. `full`, `refresh` e `classify_only`
mantêm a semântica atual; o pipeline é um modo novo (`process_backlog`).

Detalhe: a existência do arquivo de thumbnail é verificada em disco, não em SQL
— a consulta acima superestima; o filtro de existência continua no código,
como em [`list_thumb_repair_rows`](../sherlock/desktop/src-tauri/src/db.rs:1073).

### 4.5 Orçamento de CPU

`is_network_mount` já existe ([`platform/fs.rs:86`](../sherlock/desktop/src-tauri/src/platform/fs.rs:86)):

| Raiz | workers | razão |
|---|---|---|
| rede (cifs/smb/nfs/gvfs) | 2 | 4 workers já saturam o link medido |
| disco local | `min(cores - 2, 4)` | deixa CPU para o pré-processador do Ollama |

Nunca `cores` workers: a CPU precisa atender o `gap` do Ollama, que é o caminho
crítico do request.

## 5. Fase 0 — medição (o gate)

Instrumentação (o que foi implementado primeiro):

1. `vision_prep::prepare` mede **decode**, **resize** e **encode**
   separadamente, em `log::debug!`.
2. Um teste `#[ignore]` (`bench_cost_split`) mede, por arquivo e na ordem do
   scan: `read` (primeiro toque), `decode` puro (`load_from_memory`), `dhash`,
   `thumb` total, `prepare` total, `image::open` (para comparar a leitura via
   `BufReader`) e, com `BENCH_MODEL`, `request`/`prefill`/`decode_gpu`/`ollama_cpu`.
3. `classify::probe_primary` já devolve `wall_s` + `prefill`/`decode`; o `gap`
   passa a ser dividido em `prepare` (nosso) e `ollama` (deles).
4. `BENCH_ROOT` aceita caminho real **ou** URI `smb://host/share/...`, mapeada
   para o mount GVFS que o app indexa (`$XDG_RUNTIME_DIR/gvfs/smb-share:...`).

Como rodar (o bench amostra a raiz em tercis de tamanho, não aleatoriamente):

```bash
cd sherlock/desktop/src-tauri
BENCH_ROOT=/caminho/da/raiz BENCH_LIMIT=200 BENCH_MODEL=qwen2.5vl:3b \
  cargo test --release bench_cost_split -- --ignored --nocapture
```

`BENCH_PHOTOS` (arquivo com um caminho por linha) tem precedência sobre `BENCH_ROOT`;
`BENCH_MAX_SIDE` e `BENCH_PROFILE` sobrescrevem a configuração. Sem `BENCH_MODEL` só as
colunas de CPU/IO são medidas. A unidade duplicada é `read + decode` — o primeiro toque
(que o cache de página local não esconde) mais a decompressão pura.

Protocolo:

- ~200 arquivos reais, 3 tercis de tamanho, na mesma raiz de produção;
- 3 rodadas: (a) scan sequencial atual, (b) reparo de thumb, (c) `classify_only`;
- modelo e `image_max_side` registrados; nada de constante tirada desta máquina.
- registrar em [`_bench_vision/RESULTS.md`](../_bench_vision/RESULTS.md) §7.

Critério de decisão (registrar em [`_bench_vision/RESULTS.md`](../_bench_vision/RESULTS.md)):

| Medida | Decisão |
|---|---|
| teto do pipeline ≥ 20 % do sequencial **e** produtor < consumidor | **segue** para a Fase 1/2 (mesmo com duplicado < 20 %) |
| teto do pipeline < 20 % | **para**; só vale a paralelização da fase 2 |
| produtor ≥ consumidor (não há folga de CPU) | **para**; os workers não se pagam |
| pipeline A/B ≥ 20 % mais rápido que o sequencial | **liga** o pipeline por padrão |
| pipeline A/B mais lento, ou latência por imagem pior no 3b | **desliga**; fica só o decode compartilhado |

O denominador importa: comparar o duplicado com a CPU isolada (sem `request`)
infla o percentual. Medir por **classe de tamanho** — a mediana de uma pasta de
JPEG de WhatsApp não representa um NEF de 14 MP.

### 5.1 Primeiros números (3 arquivos, GVFS/SMB, sem modelo)

| Medida | Valor |
|---|---|
| read (1º toque) | 22,1 ms |
| decode puro | 10,2 ms |
| dhash | 5,0 ms |
| thumb total | 76,6 ms |
| prepare total | **195,4 ms** |
| `image::open` | 30,6 ms (95 % de read+decode) |
| duplicado (read+decode) | 32,3 ms |

1. **`image::open` não é o problema**: 95 % de `read + load_from_memory`. A
   estratégia de leitura não precisa mudar.
2. **`prepare` é a etapa mais cara**: 2,5× a thumbnail e 6× o read+decode. Não é
   I/O nem decode — é Lanczos3 + `to_rgb8` + encode q88 + o `stat`/EXIF extra que
   ele faz por conta própria. A thumbnail usa o filtro box de
   `imageops::thumbnail` e custa uma fração.
3. **Para JPEG pequeno o pipeline não paga**: 32 ms de duplicado contra um
   request de ~3,5 s (3b, 1024 px) é ~1 %. O gate só fecha positivo na classe de
   arquivos grandes, onde read+decode chega a segundos.

### 5.2 Medição real (174 arquivos, 1,41 MB de média, qwen2.5vl:3b, 16 vCPU)

| Medida | Valor |
|---|---|
| read (1º toque) | 136,3 ms |
| decode puro | 50,7 ms |
| dhash | 81,3 ms |
| thumb total | 300,3 ms |
| prepare total | **606,7 ms** |
| `image::open` | 154,3 ms (82 % de read+decode) |
| request | 2105,6 ms (prefill 300,5 + decode GPU 285,5 + **CPU 1519,6**) |
| sequencial | 3012,6 ms |
| duplicado | 187,1 ms — **6 %** |
| teto do pipeline | 2239,5 ms — **26 %** |

1. O critério original ("duplicado ≥ 20 %") **falha**, com 6 %. Ele media só a
   remoção do decode e ignorava a sobreposição, que é o objetivo: o request tem
   1519,6 ms de CPU com a GPU ociosa. Usar essa janela para adiantar o próximo
   arquivo vale até 20 dos 26 pontos.
2. **Há folga, não contenção**: o produtor gasta 907 ms (thumb+prepare) contra
   2105,6 ms do consumidor — 2,3× de margem, com 16 vCPU para o pré-processamento
   do Ollama. Um produtor basta; não é preciso brigar por CPU.
3. **`prepare` é 20 % do custo total** — mais que o decode compartilhado. Cortar
   o filtro é o maior ganho isolado e não depende de concorrência: vem primeiro.
4. `image::open` é 18 % **mais barato** que `read + load_from_memory` (154 vs
   187 ms). O decode compartilhado deve usar `image::open` uma vez e reusar o
   `DynamicImage`; "ler tudo para memória" seria um retrocesso.
5. `dhash` custa 81,3 ms (full-res → 9×8). A partir da thumb de 300 px custaria
   ~1 ms, mas muda os hashes já gravados e degrada o dedupe até regenerar —
   candidato posterior, fora desta iteração.
6. **NEF não entra**: os arquivos de 14 MB não são lidos (fora de
   `SUPPORTED_EXTS`, e o `image` não decodifica NEF). Se entrarem, o duplicado
   sobe para ~2,4 s/arquivo e o pipeline passa a valer muito mais que 26 %.

### 5.3 Fase 0.5 — filtro do downscale (implementado)

Mesma raiz, 6 arquivos, sem modelo:

| | ms/arquivo |
|---|---|
| `prepare` box | 383,8 |
| `prepare` Lanczos3 | 571,4 |
| diferença | **−187,6 (−33 %)** |

Contra o total por arquivo de §5.2 (3012,6 ms), são **−6,2 %** sem nenhuma
mudança de concorrência, e o produtor do pipeline cai de 907 ms para ~720 ms.

`settings.toml: image_filter` aceita `auto` (padrão: box para JPEG/HEIC,
Lanczos3 para PNG/BMP/TIFF), `box` e `lanczos`. A calibração do estimador passou
a incluir o filtro na chave de cache, então mudar o valor invalida a medição
antiga em vez de superestimar a velocidade.

Falta: validar a qualidade em uma pasta com screenshots/texto fino
(`BENCH_QUALITY=1` imprime as duas respostas lado a lado) e expor a opção no
modal de configurações — hoje só existe em `settings.toml`.

## 6. Riscos

| Risco | Mitigação |
|---|---|
| Contenção de CPU piora a latência por imagem | workers 2, reserva de 2 cores, gate A/B |
| Ler o mesmo arquivo 2× no SMB (se o produtor e o consumidor rodarem para linhas diferentes) | backlog combinado garante 1 read por arquivo |
| Cache de JPEG preparado cresce sem limite | default transiente; se persistente, apagar após classificar |
| Dedupe muda de representante | documentado; cluster e resultado continuam corretos |
| Qualidade: usar a imagem preparada em vez do original | `prepare` já é o que vai ao modelo hoje — bytes idênticos |
| Vídeo/PDF quebram com a mudança | fora do escopo da 1ª iteração |

## 7. Testes

- `decode_source` + `thumbnail_from_decoded` produzem **byte-idêntico** ao
  `generate_thumbnail` atual (mesma imagem, mesmo dHash).
- `prepare_from_decoded` produz o mesmo JPEG que `prepare` para o mesmo
  `max_side` (mesma dimensão, mesmo hash de dHash do resultado).
- Pipeline: 20 imagens, 2 produtores, 1 consumidor ⇒ todas thumbnailed e
  classificadas; cancelar no meio ⇒ nenhuma linha parcial.
- Backlog combinado: linha com thumb presente e `confidence = 0` é classificada
  sem gerar thumb; linha com thumb ausente e classificada só gera thumb.
- Regressão: `repair_missing_thumbnails` e `classify_only` continuam passando.

## 8. Perguntas abertas

1. Cache de JPEG preparado: transiente ou persistente? (default proposto:
   transiente; persistente se o resume de `classify_only` for caso de uso real)
2. Vale a pena um botão "Processar backlog" ou o pipeline entra só no `full`?
3. Vídeo (1 vs 5 keyframes) é a próxima iteração ou fica fora de escopo?
4. O filtro do resize em `prepare` deve ser o box de `imageops::thumbnail` (como
   na thumbnail) ou Lanczos3? 195 ms contra 77 ms medidos; só um teste de
   qualidade em texto fino decide — e é uma otimização independente do pipeline.
