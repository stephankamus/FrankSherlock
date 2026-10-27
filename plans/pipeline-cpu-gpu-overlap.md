# Plano — pipeline CPU/GPU: um decode, dois artefatos

> **Status: Fases 0, 0.5, 1 e 2 implementadas.** Falta a Fase 3: o A/B de ponta a
> ponta que confirma (ou derruba) o teto de 26 % medido na Fase 0.
>
> Entregue: bench de custo (§5), filtro `image_filter` (−33 % no `prepare`, §5.3),
> decode compartilhado (−46 % nos dois estágios de CPU, §5.4), o modo `backlog`
> com produtor/consumidor (§4.3) e botão na sidebar, e o corte de §5.12 (`full` =
> descoberta + pipeline, com a fase 2 restrita ao `refresh`).
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
| Workers de CPU | **1 produtor** | medido: 600 ms contra 2 105 ms do consumidor; mais threads só brigariam com o Ollama |
| Gate | teto do pipeline < 20 % **ou** produtor ≥ consumidor ⇒ **descartar o pipeline** | requisito explícito do usuário: não pode ficar mais lento. O duplicado isolado não serve de critério: ele ignora a sobreposição, que é o objetivo |

## 4. Desenho

### 4.1 Um decode, dois artefatos (implementado)

`thumbnail.rs` expõe o decode intermediário:

```rust
/// Decodifica, aplica orientação EXIF e calcula o dHash uma única vez.
pub struct DecodedSource {
    pub image: image::DynamicImage, // orientação já aplicada
    pub dhash: u64,
    pub orientation: u16,           // valor do arquivo; Ollama ignora EXIF
}

pub fn decode_source(path: &Path) -> Option<DecodedSource>;

pub fn thumbnail_from_decoded(
    decoded: &DecodedSource,
    thumb_dir: &Path,
    rel_path: &str,
) -> Option<ThumbnailResult>;
```

`generate_thumbnail` virou um wrapper (`decode_source` + `thumbnail_from_decoded`),
então o reparo existente não mudou. `downscale` passou a receber `&DynamicImage`:
clonar um buffer de 12–40 MP só para encolhê-lo devolveria o ganho.
Teste de equivalência garante que os dois caminhos produzem o **mesmo JPEG** e o
mesmo dHash.

### 4.2 JPEG de visão sem reler a origem

```rust
/// Escreve o JPEG de `max_side` (q88) a partir de uma imagem já decodificada.
/// `prepare_with` continua sendo o caminho de quem lê do disco.
pub fn prepare_from_decoded(
    source: &Path,
    original_bytes: u64,   // evita um `stat()` extra no SMB
    orientation: u16,      // Ollama ignora EXIF: JPEG rotacionado não passa direto
    decoded: &image::DynamicImage, // orientação já aplicada
    max_side: u32,
    tmp_dir: &Path,
    filter: ImageFilter,
) -> AppResult<PreparedImage>;
```

Mesmos fast paths de `prepare_with` (original intocado quando `max_side == 0`,
e quando é um JPEG que já cabe e está upright), agora decididos sem tocar no
disco. Teste de equivalência garante o mesmo resultado que `prepare_with`.

Ciclo de vida do artefato (decisão na Fase 0, ver §5):

- `transitório` (default proposto): arquivo temporário no diretório do
  pipeline, apagado pelo consumidor. Disco limitado pela profundidade da fila.
- `persistente` (opcional): `cache/prepared/<root_key>/<rel_path>.jpg`, apagado
  quando a linha é classificada; entra em `purge_root` e `cleanup_deleted_caches`.
  Torna o resume de `classify_only` instantâneo, ao custo de ~200 KB × pendentes.
- Chave do cache: `image_max_side` + qualidade JPEG. Mudar a resolução no modal
  invalida tudo.

### 4.3 Pipeline produtor/consumidor (implementado)

```
rows do backlog (ordenados por rel_path)
        │
        └─ produtor (1 thread) ─► fila limitada (4) ─► consumidor (GPU, thread atual)
              leitura SMB, decode, dHash,               (row, dhash, prepared?)
              thumb 300, JPEG para o modelo                     │
                                                                ├─ dedupe por dhash
                                                                └─ classify + UPDATE
```

- **Um** produtor, não N: medido, ele custa ~600 ms por arquivo contra os ~2,1 s
  do consumidor — 3,5× de folga. Mais threads só competiriam com o
  pré-processamento do Ollama.
- Fila limitada (`sync_channel(4)`): o produtor para quando o consumidor atrasa,
  então no máximo 4 JPEGs preparados existem em disco e só 1 bitmap decodificado
  fica em memória (o produtor solta o `DynamicImage` antes de bloquear no `send`).
- Cancelamento: o produtor para no `AtomicBool`; o consumidor sai do laço e
  **fecha o receptor**, o que desbloqueia um `send` pendente. Sem isso o
  `thread::scope` travaria no join.
- Retomada: a seleção é refeita a partir do índice, então o passe é idempotente e
  não grava cursor. `checkpoint_scan_job` roda a cada 25 arquivos só para a UI.
- Progresso: `ScanJobStatus` só tem um contador, e escrever dois a mais no SQLite
  por arquivo custaria uma escrita por request. O pipeline reporta por um canal
  em memória (`BacklogTick` → `models::BacklogProgress`), pollado pela sidebar:
  **thumbnails** e **classified**, cada um com o seu percentual, mais
  `processed/total`, `deduped` e `failed`.
- Denominadores separados (`thumbs_total`, `classify_total`): a maioria das
  linhas precisa de só uma das duas etapas. Dividir pelo backlog inteiro fazia o
  contador de thumbnails parecer travado em 0 % depois de um
  `Repair Thumbnails` que já tinha enchido o cache — o passe não está quebrado,
  simplesmente não havia thumbnail faltando. Quando nenhuma precisa, a UI diz
  "none missing" em vez de "0 %". `thumbs_total` cresce se o produtor encontrar
  ponteiro morto (só o `stat` revela).
- Parar: o botão usa o `cancel_scan(job_id)` que já existe (um passe de backlog é
  um scan job), então o cancelamento e o desbloqueio do produtor são os mesmos
  de qualquer scan.
- Retomada após interrupção: o `phase` do job fica gravado como `backlog`, e o
  `onResumeScan` do frontend usa esse valor para retomar no modo certo em vez de
  virar um `full`.

### 4.4 Backlog combinado (implementado)

Seleção única em vez de dois passes:

```sql
SELECT ... FROM files
WHERE deleted_at IS NULL
  AND (confidence = 0 OR thumb_path IS NULL)
ORDER BY rel_path
```

`full`, `refresh` e `classify_only` mantêm a semântica atual; o pipeline é um
modo novo (`backlog`), com botão "Process Backlog" na sidebar.

Dois limites, ambos deliberados:

1. **Ponteiro morto não aparece na consulta.** Descobrir que `thumb_path` aponta
   para um arquivo inexistente exige um `stat` por linha. O produtor faz esse
   `stat` nas linhas que já vai ler (e regenera a thumbnail se estiver morta),
   mas uma linha **classificada** com thumbnail morta continua sendo trabalho do
   `repair_thumbnails`.
2. **Vídeo e PDF não passam pelo pipeline** nesta iteração: o consumidor chama o
   caminho antigo (`classify_and_update`), que reextrai keyframes/páginas.
   A thumbnail deles também continua com o reparo.

### 4.5 Orçamento de CPU

Medido: **um** produtor entrega 3,5× mais rápido do que o consumidor consome, em
16 vCPU. Não há orçamento a calibrar — o gargalo é a GPU, e o produtor fica
bloqueado na fila a maior parte do tempo. Se um dia a fila ficar vazia de forma
recorrente (arquivos pequenos em disco local), aí sim vale medir um segundo
produtor por [`is_network_mount`](../sherlock/desktop/src-tauri/src/platform/fs.rs:86).

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

### 5.4 Fase 1 — decode compartilhado (implementado)

| Caminho | ms/arquivo |
|---|---|
| `generate_thumbnail` + `prepare` (2 decodes) | 1061,9 |
| `decode_source` + `thumbnail_from_decoded` + `prepare_from_decoded` | **578,0** |
| diferença | **−483,9 (−46 %)** |

O caminho compartilhado corta um read, um decode, um `extract_orientation` (que
abre o arquivo de novo) e um `stat` por arquivo. Contra o total de §5.2
(3012,6 ms), o efeito fica entre **−10 % e −16 %** — a variabilidade do NAS entre
rodadas na mesma pasta chega a 50 %, então só o delta dentro da mesma rodada vale.

Ciclo de vida do artefato (decisão tomada): **transiente**. `prepare_from_decoded`
escreve em `tmp_path` e `PreparedImage::drop` apaga; nada de cache persistente
nesta fase. A variante persistente continua em aberto (§8.1) e é o que tornaria
o resume de `classify_only` instantâneo.

### 5.5 Diagnóstico de uma noite de backlog (10 h, 8.102 classificados)

Sintoma: depois de ~10 h, cada arquivo passou a levar 8–25 s em vez dos ~2–4 s
esperados. A investigação foi no journal do Ollama, não no código do pipeline:

| Evidência | Valor |
|---|---|
| `starting runner` em 15 min | **356** |
| cargas com `KvSize:2048` / `KvSize:4096` | 162 / 154, em **blocos de 3** |
| `BatchSize` / `FlashAttention` | 512 / **Disabled** |
| `compute graph` (CUDA0) | **7,3 GiB** + 3,0 GiB de pesos em 12 GB |
| VRAM livre no momento da falha | 610 MB |
| erro do runner | `terminate called ... std::runtime_error`, `exit status 2` |

Duas causas, as duas nossas:

1. **`num_ctx` alternando.** A passada primária (180 tokens de saída) resolvia
   2048; o OCR do LLM (1500) resolvia 4096. Cada troca de `num_ctx` faz o Ollama
   derrubar o runner e recarregar o modelo (~2,1 s + realocação de 7,3 GiB).
   Correção: [`NUM_PREDICT_RESERVE = 1500`](../sherlock/desktop/src-tauri/src/llm/client.rs:85)
   no cálculo e `MIN_NUM_CTX = 4096`, então **todo** request de uma execução usa
   o mesmo contexto. Custo: ~144 MiB de KV.
2. **O runner morria** quando a alocação não cabia (7,3 GiB de grafo + 3 GiB de
   pesos + desktop ≈ 11 GB de 12 GB). Correção de robustez: *circuit breaker* no
   consumidor — 25 falhas consecutivas sem nenhum sucesso abortam o job com erro
   explícito em vez de moer milhares de arquivos (antes: 10 h de trabalho perdido
   sem aviso).

Ajustes recomendados **na máquina**, não no código:

```ini
# /etc/systemd/system/ollama.service.d/override.conf
[Service]
Environment="OLLAMA_FLASH_ATTENTION=1"   # reduz grafo e KV — confirmado no journal
```
E remover `OLLAMA_CONTEXT_LENGTH=16384`, que hoje está no drop-in: o app sempre
envia `num_ctx`, mas qualquer request que esqueça herda 16k e estoura a VRAM.

`OLLAMA_NUM_BATCH=128` **não funcionou**: com o drop-in aplicado o journal
continuou registrando `BatchSize:512`. O batch é opção **por request**
(`options.num_batch`), então agora vai no corpo de cada chamada — ver §5.6.

### 5.6 A primeira passada A/B de verdade (191 arquivos, `full`)

Primeira execução do braço A com o binário novo, medida por
[`scripts/measure-run.py`](../scripts/measure-run.py:1) já lendo índice + journal:

| Métrica | Valor |
|---|---|
| wall | 8m10s (490 s) para 191 arquivos = **2,57 s/arquivo** |
| requisições `/api/generate` | 272 (1,42 por arquivo) |
| duração por requisição p50 / p95 / máx | 1,09 s / 4,13 s / 13,70 s |
| tempo total de GPU | 453 s (**92% do wall**) |
| respostas 500 | 6 |
| cargas de modelo / spawns de runner | 6 / **138** |
| `KvSize` | 4096 em todas as 6 cargas ✅ (a correção do §5.5 funciona) |
| `BatchSize` | 512 em todas — o env não pega |
| VRAM livre no pior momento | 534 MB de 12 GB |
| linhas `Classifying:` no log | 23 de 191 |

Três coisas apareceram nessa medição:

1. **`confidence = 0` não é "não classificado".** As 191 linhas foram
   classificadas (273 requests, 8 min) e **112 ficaram com `confidence = 0`**
   porque é o número que o *modelo* devolve — o template do prompt mostra
   literalmente `"confidence":0.0` e o qwen2.5-VL responde 0.0 para descrições
   boas. Como todo o banco usava `confidence = 0` como "pendente", essas 112
   voltavam para a fila em toda passada e eram reenviadas à GPU para sempre (e
   inflavam os contadores do backlog e o prompt de startup).
   Correção: [`PENDING_CLASSIFICATION_SQL`](../sherlock/desktop/src-tauri/src/db.rs:680)
   — pendente é não ter `description`, que toda classificação bem-sucedida
   escreve e que o registro-placeholder de arquivo novo/modificado limpa.
2. **O log do app é truncado a cada start**, então medir pelo tail do log perde
   a execução (23 de 191 linhas). O script agora tira o essencial do índice
   (linhas do job, quantas ainda estão pendentes) e do journal do Ollama
   (requisições, duração, cargas, crashes); o log virou detalhe.
3. **O runner continua morrendo** com `BatchSize:512`: 6 `SIGABRT` em 8 min e
   138 spawns de runner para 6 cargas. Correção no código:
   [`NUM_BATCH = 128`](../sherlock/desktop/src-tauri/src/llm/client.rs:74) em
   `options_payload`, com teste. Rodou de novo com 128: 99 spawns e **5 crashes**
   — melhorou o estrago, não resolveu a causa (§5.7).

Consequência para o A/B: essa execução **não vale como braço A** — 92% do wall é
GPU e 6 requisições se perderam. Refaça depois de resolvido o §5.7.

### 5.7 A causa real dos crashes: a gramática do JSON do Ollama

O journal respondeu o que o §5.5 tinha atribuído a VRAM. **Todos** os crashes do
dia têm a mesma assinatura, 236 vezes:

```
terminate called after throwing an instance of 'std::runtime_error'
  what():  Unexpected empty grammar stack after accepting piece: ! (0)
SIGABRT: abort
```

Não é alocação de VRAM (no `fit` havia 11,1 GiB livres), não é `num_ctx` (todas as
cargas em 4096) e não é `BatchSize` (acontece igual em 512 e em 128). É o
**sampler com gramática** — `format: "json"`, que mandamos em
[`options_payload`](../sherlock/desktop/src-tauri/src/llm/client.rs:74) via
`json_mode` — abortando quando o modelo tenta emitir um token que a gramática não
aceita. Cada aborto mata o runner, perde a requisição (500) e recarrega ~7,3 GiB.

**Implementado:** o request agora tem uma segunda chance. Quando a primeira
tentativa falha de um jeito que só pode ser o runner morrendo — 5xx, erro de
transporte (sem status) ou corpo da resposta interrompido —
[`AttemptError::worth_retrying`](../sherlock/desktop/src-tauri/src/llm/client.rs:175)
libera um retry **sem** `format: "json"`; um 400 e um timeout não são retentados.
O crash continua custando o reload — o que já aconteceria —, mas a classificação
**não se perde**. O corpo do erro também passou a ser lido e logado (ureq o
descartava), então a próxima falha vem com o motivo escrito.

### 5.7.1 Ollama 0.40.11 mudou a forma do problema

Atualizado de 0.23.1 para **0.40.11**, três fatos novos:

1. O probe
   ([`scripts/probe-json-grammar.py`](../scripts/probe-json-grammar.py:1)) rodou
   **360 + 560 requests** alternando com e sem gramática, sobre 4 imagens: **zero
   falhas nos dois braços**, zero `unsalvageable`. Ambas as versões do output
   parseiam. Latência empatada (~0,61 s) — a diferença de 4,6 s vs 2,4 s vista no
   smoke test era o carregamento inicial do modelo.
2. Mesmo assim, a passada real no app deu **94 respostas 500 em 275 requests
   (34%)** no mesmo modelo, sem nenhum `runner crashes` — o Ollama devolve 500 em
   vez de matar o runner. O journal não registra mais nada além da linha GIN; o
   motivo agora só aparece no log do app.
3. O probe não reproduz a falha porque o request do app é mais pesado: prompt com
   mais chaves e `num_predict = 180`. A hipótese viva é a gramática batendo no
   limite de tokens e não conseguindo fechar o objeto — mas isso agora é
   verificável, não adivinhado: o corpo do 500 vai para o log.

**Por que nada disso aparecia no log:** [`classify_primary`](../sherlock/desktop/src-tauri/src/classify.rs:418)
já tenta **duas** vezes — a segunda com `json_mode: false` e um prompt de fallback —
e descartava os dois erros em silêncio. É daí que vem `requests per file ≈ 1.6`, e
é por isso que o retry não disparava: a tentativa que falha já é a sem gramática,
não há para onde degradar. As três tentativas (primária json, fallback, e o OCR do
LLM) agora logam o motivo de cada falha, e o corpo do 500 vem junto.

### 5.7.2 A causa: o prompt de fallback entrava em loop

Com o log consertado (`status 500: …`), a resposta apareceu em uma linha:

```
primary (fallback) failed for vp_31582c9f3aa0cb64.jpg:
  http_error: status 500: prediction aborted, token repeat limit reached
```

E reproduzido fora do app, na mesma imagem preparada (768 px, BOX, q88):

| prompt | sem gramática | com gramática |
|---|---|---|
| `PRIMARY_PROMPT_CONCISE` | 200, JSON válido | 200, JSON válido |
| `PRIMARY_PROMPT_FALLBACK` | **500 token repeat limit** | (não testado) |

Ou seja: o **texto do retry** era a causa do problema que ele existia para
resolver. Nas fotos de obra (concreto, baixo contraste) essa redação faz o
qwen2.5-VL entrar em repetição, e o Ollama 0.40 aborta com 500 em vez de emitir o
lixo. Temperatura 0,3 e `repeat_penalty` 1,15/256 **não** quebram o loop — testado,
500 nos três casos.

Correção: o retry agora mantém o prompt e muda **só** o modo de amostragem
(`json_mode: false`); `PRIMARY_PROMPT_FALLBACK` foi removido.

### 5.7.3 O que sobrou (e é pior do que parecia)

Com o prompt trocado, os 107 continuaram falhando. O log mostrou dois sinais
diferentes para o mesmo arquivo:

```
Ollama answered 200 with an empty response (done=false, model_echo="");
  sent model="qwen2.5vl:3b" prompt=430 B image=101000 B body=101500 B
  body: {"model":"","created_at":"0001-01-01T00:00:00Z","response":"","done":false}

primary (fallback) failed: http_error: status 500: prediction aborted, token repeat limit reached
```

`model: ""` e `created_at` zerado é o struct **vazio** do Go: o Ollama respondeu
como se nunca tivesse recebido o request. E o loop de repetição não é
determinístico — o mesmo prompt, que deu 500 em três tentativas seguidas, deu 200
com JSON válido minutos depois, e a requisição feita **fora do app** nunca falhou.

O que entrou para fechar isso em um run só:

1. **Terceira tentativa**: mesmo prompt, sem gramática, `temperature = 0.3` e
   `num_predict` maior. Se o loop é sorte de amostragem, uma amostra diferente
   escapa.
2. A imagem preparada que falhou é **preservada** em `cache/tmp/keep_vp_*.jpg`,
   com tamanho e dimensões no log — sem ela não há como reproduzir fora do app.
3. O aviso de resposta vazia agora imprime **o que foi enviado** (model, tamanho
   do prompt, da imagem e do corpo). Se o request chegou intacto, os números
   mostram; se não, `model: ""` deixa de ser mistério.

Se a terceira tentativa também falhar com o `keep_*.jpg` na mão, o próximo passo é
tentar o **arquivo original** em vez do preparado para aquela imagem, que separa
"imagem preparada é o gatilho" de "o modelo trava nessa foto".

### 5.7.4 A terceira tentativa falhou — e a imagem não é a culpada

Resultado com as três tentativas: os mesmos 107 arquivos. Mas os `keep_*.jpg`
deixaram a investigação avançar de forma decisiva. Enviando **os bytes exatos que
o app mandou** (`keep_vp_*.jpg`, 43–142 KB, 432×768, PIL decodifica):

| variação | resultado |
|---|---|
| prompt real + `format: json` | 200, JSON válido |
| prompt real, sem gramática | 200, JSON válido |
| prompt real, sem gramática, `temperature 0.3` / 260 tokens | 200, JSON válido |

E o resto do ambiente também foi descartado: um único Ollama na máquina
(0.40.11, só `127.0.0.1`, `localhost` resolve para ele), e nenhuma interferência
do polling da UI (`/api/ps` a cada 200 ms durante 12 requisições: 12/12 ok).

Sobra **o cliente HTTP do app**. Duas mudanças para fechar isso:

1. O corpo agora é serializado por nós, compacto, e enviado como bytes
   (`serde_json::to_vec` + `.send(&body)`), sem passar pelo caminho interno do
   `ureq`. O corpo é idêntico ao de um `curl --data-binary`.
2. [`scripts/ollama-proxy.py`](../scripts/ollama-proxy.py:1) grava e repassa cada
   requisição; `SHERLOCK_OLLAMA_BASE=http://127.0.0.1:11435` aponta o app para
   ele sem recompilar.

### 5.7.5 A captura resolveu: o runner **trava** e o resto da passada morre junto

O proxy entregou 408 requisições. Reenviando `req_0001.json` **byte a byte** pela
rede, obtive exatamente o mesmo `{"model":"","created_at":"0001-01-01T00:00:00Z",
"response":"","done":false}`. Então o corpo não era o problema — o **estado** era.

A linha do tempo do log fechou o caso:

| instante | o que aconteceu |
|---|---|
| 18:12:37 | começa a classificação; os primeiros ~84 arquivos classificam normalmente |
| 18:14:39 | **primeira** resposta vazia |
| até 18:20:03 | **todas** as requisições seguintes falham (107 arquivos × 3 tentativas) |

Depois da primeira falha o runner nunca mais responde: `model:""` na tentativa
com gramática e `500 prediction aborted, token repeat limit reached` nas duas sem
gramática. Reenviar os mesmos bytes minutos depois, com o runner recarregado,
funciona — o que explica por que nada reproduzia fora do app e por que **sempre os
mesmos 107 arquivos** falhavam: eles são o fim da ordem alfabética, depois do
ponto onde o runner trava. Na passada seguinte são eles os únicos pendentes, então
o travamento chega logo no começo e todos falham.

Correção: quando a resposta é o frame vazio do Ollama, é **reload**, não retry —
[`management::reload_model`](sherlock/desktop/src-tauri/src/llm/management.rs:153)
descarrega o modelo (`keep_alive: 0`, throttled a 10 s) e a requisição é repetida
num runner novo. É um bug do Ollama 0.40.11 (motor `llama-server`), não nosso; a
mitigação é do nosso lado até o upstream corrigir.

### 5.7.6 … e destapou um bug de dados no scan

Efeito colateral do "Reclassify" com a opção de thumbnails: reclassificar →
`Rescan` fazia a pasta **desaparecer** (a sidebar mostrava "0 files") e só um
segundo `Rescan` a recuperava.

Causa: os arquivos ficam `Unchanged` (o fingerprint não muda), mas entram no
conjunto `heal` porque o ponteiro do thumbnail é NULL. E o flush dos marcadores
**exclui** os healed:

```rust
.filter(|p| !heal.contains(&p.rel_path))   // heal carimba por conta própria…
```

…só que o caminho de heal chamava `update_file_thumb_and_dhash`, que **não toca em
`scan_marker`**. Resultado: a fase 4 comparava o marcador antigo, considerava todas
as linhas ausentes e fazia soft-delete de tudo — apagando também os thumbnails.
O bug existia antes (qualquer thumbnail perdido dispara o heal), mas o Reclassify
com a opção ligada o tornava trivial de acionar.

Correção: os dois caminhos de heal carimbam o marcador
([`scan.rs`](../sherlock/desktop/src-tauri/src/scan.rs:561)), com teste de regressão
`healed_rows_survive_the_end_of_scan_cleanup` — verificado nos dois sentidos:
desligando o carimbo o teste falha com "the healed row must survive the cleanup".

**Bug de dados encontrado no caminho:** quando a requisição falhava, o
`ClassificationResult` vazio era escrito por cima da linha, apagando a descrição
que já existia (107 de 191 linhas ficaram sem descrição numa passada). Corrigido:
[`is_empty_result`](../sherlock/desktop/src-tauri/src/classify.rs:288) barra a
escrita, o job conta as falhas (`… classification complete (191 files, 25 deduped,
N failed)`) e só uma classificação bem-sucedida alimenta o índice de dedupe.

Opções para atacar a causa, em ordem de custo:

1. **Medir antes de decidir**: script de probe com N requests por braço
   (com/sem `format: "json"`), contando 500, falhas de parse e latência. A taxa
   observada é ~2% dos requests, então ~150 por braço já dá sinal.
2. Se o probe confirmar, **parar de mandar `format: "json"`** e confiar nos prompts
   ("No markdown. No extra keys.") + no parser tolerante que já existe
   ([`extract_first_json_object`](../sherlock/desktop/src-tauri/src/llm/client.rs:235)
   e [`salvage_primary_from_raw`](../sherlock/desktop/src-tauri/src/classify.rs:118)).
   Antes de ligar como padrão, comparar as descrições dos mesmos 191 arquivos com
   as da versão com gramática.
3. Passar um **schema** em vez do `json` solto: continua usando o mesmo motor de
   gramática, então provavelmente não resolve.
4. Atualizar o Ollama (0.23.1) e torcer para o bug ter sido corrigido.

Enquanto isso, `runner crashes > 0` no `measure-run.py` invalida o braço.

### 5.8 A passada de 25/10 medida de verdade (886 arquivos, raiz `Fotos`)

Primeira medida em que o resumo descreve **uma** execução. O que ele dizia antes:

| | resumo antigo | janela real |
|---|---|---|
| wall | 4543h16m31s (189 dias) | **1h25m12s (5112 s)** |
| requests | 35 124 (40,1 por arquivo) | **2045 (2,34 por arquivo)** |
| model loads / spawns / crashes | 5875 / 41 649 / 251 | **0 / 0 / 0** |
| `KvSize` | 16384, 2048, 4096, 8192 | **4096 em todas** |
| `BatchSize` | 512 e 128 | **128 em todas** |

Duas causas, as duas de contabilidade:

1. `scan_jobs.started_at` era herdado na retomada ([`db.rs`](../sherlock/desktop/src-tauri/src/db.rs:575)). O job #2 nasceu em **abril** e foi retomado em 25/10, então o "wall" e a janela do journal cobriam seis meses de tráfego alheio. Correção: a retomada carimba `started_at` novo (com `CASE` para não mover o início de um job que já está rodando).
2. `scripts/measure-run.py` usava essa coluna como janela ([`measure-run.py`](../scripts/measure-run.py:257)). Agora, quando o intervalo é impossível (> 24 h), ele busca 48 h de journal, deriva o início do último bloco contínuo de `/api/generate` e **recorta** as linhas fora dele.

Números da passada, agora comparáveis com §8.1:

| métrica | raiz `Fotos` (886 arq.) | pasta `2020` (191 arq.) |
|---|---|---|
| wall | 5112 s — **5,84 s/arquivo** | 423 s — 2,21 s/arquivo |
| requests | 2045 (2,34/arquivo) | 303 (1,59/arquivo) |
| 200 / 500 / 400 | 1840 / 186 / 19 | 276 / 26 / 1 |
| p50 / p95 / máx | 1,29 / **12,64** / 13,83 s | 1,26 / 3,55 / 12,69 s |
| tempo de GPU | 4792 s = **94 % do wall** | 414 s = 98 % |
| deduped / failed | 104 / 63 | 24 / 9 |

Três leituras:

1. **O runner está estável.** Zero cargas, zero spawns, zero crashes, um único `KvSize` e um único `BatchSize` na passada inteira: as correções de §5.5–§5.7.5 valem. O aviso de "runner sendo retentado em loop" do resumo antigo era falso positivo.
2. **O pipeline não tem mais CPU para esconder.** 94 % do wall já é requisição; a conclusão de §8.1 fica mais forte, não mais fraca.
3. **A cauda não é falha, é OCR.** Os 133 requests acima de 12 s são todos **200** e somam 1741 s = **36 % do tempo de GPU**. Eles param no mesmo platô de ~13 s, que é `run_llm_ocr` com `NUM_PREDICT = 1500` ([`classify.rs`](../sherlock/desktop/src-tauri/src/classify.rs:736)) contra os 180 da classificação. ~15 % da biblioteca dispara enriquecimento de documento.

E o que sobrou de falha real:

- **186 respostas 500** (`prediction aborted, token repeat limit reached`): 8,5 % dos requests, 240 s. Barato em GPU, caro em linhas perdidas — 63 arquivos por passada voltavam para a fila.
- **19 respostas 400**, **todas** no 4º chute, com o arquivo original. O `num_ctx` era calculado para a imagem preparada de 768 px (4096) e o original precisava de 4136–4201. Aquele chute não podia dar certo: é o que [`original_fits_window`](../sherlock/desktop/src-tauri/src/classify.rs:433) agora detecta antes de gastar o request.

**Backlog real naquele momento:** 44 716 linhas sem descrição de 55 145, 9735 sem thumbnail, 45 543 com trabalho. A 5,84 s/arquivo são **~72 h contínuas**. Cortar o passe de OCR dos 15 % que o disparam é o maior ganho isolado que sobrou.

### 5.9 Blocos de contexto (`num_ctx`) em vez de reload por arquivo

`num_ctx` faz parte da identidade do runner: trocá-lo derruba o runner e recarrega o modelo (§5.5). A passada agora agrupa as linhas pela janela que **a imagem preparada vai precisar** e roda um bloco por janela ([`ctx_blocks`](../sherlock/desktop/src-tauri/src/scan.rs:1213)):

- a janela é derivada de `files.image_width/image_height` (migration 14) mais `image_max_side` e o perfil de prompt — [`primary_ctx_for`](../sherlock/desktop/src-tauri/src/classify.rs:450) usa a **mesma** [`prepared_size`](../sherlock/desktop/src-tauri/src/vision_prep.rs:75) que o `prepare` executa, então a previsão não pode divergir do real;
- blocos em ordem crescente, para o maior já estar carregado no fim;
- linha sem dimensão medida: o producer lê o header (sem decode), grava na linha e **adia** a linha se ela precisar de janela maior — o bloco dela vem na passada seguinte.

Com `image_max_side = 768` isso é **um bloco só** (é por isso que o journal de 25/10 tem um único `KvSize`). O agrupamento passa a valer com prep ≥ 1280 px ou quando a janela for elevada — e é ele que torna seguro dar um segundo chute aos arquivos que estouram 4096. Custo aceitável: a primeira passada depois da migration lê o header de cada linha sem dimensão (uma vez), e o producer continua decodificando só o que vai enviar.

### 5.10 Falha persistente e `Retry failed`

Antes, uma linha que queimava todas as tentativas voltava na passada seguinte: 63 de 886 arquivos, 3–4 requests cada, e o motivo só existia como `WARN` num log que o próximo start trunca. Agora:

- migration 15 acrescenta `classify_state`, `failure_kind`, `failure_detail`, `classify_attempts` e `failed_at` — **nunca** derivados de `description`/`confidence` (a lição de §5.6);
- [`failure_kind_from`](../sherlock/desktop/src-tauri/src/classify.rs:466) classifica a resposta em `repeat_limit`, `ctx_overflow`, `empty_frame`, `io`, `timeout`, `no_response`, `parse`; o producer distingue `decode` e `prepare` num contador separado;
- **só falhas determinísticas são persistidas** ([`is_permanent_failure`](../sherlock/desktop/src-tauri/src/scan.rs:1440)): `repeat_limit`, `ctx_overflow` e `parse`. As ambientais (`io`, `timeout`, `empty_frame`, `no_response`, `decode`, `prepare`) ficam fora do banco de propósito;
- [`list_backlog_rows`](../sherlock/desktop/src-tauri/src/db.rs:1183) **exclui** as linhas com `classify_state = 'failed'`, então a passada converge; [`list_failed_rows`](../sherlock/desktop/src-tauri/src/db.rs:1190) é a única porta de volta;
- `ScanMode::RetryFailed` roda o mesmo pipeline só com essas linhas, e aceita [`ScanOverrides`](../sherlock/desktop/src-tauri/src/models.rs:432) (modelo, `image_max_side`, perfil, engine de OCR) para tentar diferente sem mudar o `settings.toml` da biblioteca inteira;
- a barra lateral mostra **Retry N Failed** quando há falhas conhecidas, lendo `failed_classifications`.

Uma classificação bem-sucedida limpa o estado (e mantém `classify_attempts`, que é histórico), então uma linha recuperada volta a ser uma linha normal.

**Incidente de 26/10, e a regra que ele produziu.** A primeira passada com o recurso ligado durou 22 s e terminou em `failed`: 150 linhas, zero requests. A causa não era o Ollama — era o índice. Uma pasta de WhatsApp tinha 44 linhas apontando para arquivos que **não existem mais no NAS** (`No such file or directory`), sobra de uma varredura de abril. A primeira versão marcava essas linhas como falha permanente *e* contava cada uma no circuit breaker, que então abortou a passada com "Ollama is down ou o runner morre". Duas correções:

1. falha de leitura local não é falha de classificação: não vai para o banco (senão a linha desaparece do backlog até um rescan) e tem contador próprio (`unreadable`), que não alimenta o breaker;
2. o resumo ganhou a linha `N unreadable` e um aviso explícito: se o arquivo não existe mais, o caminho é **Refresh**, que faz a descoberta e derruba as linhas mortas.

O que fica: `mark_failed` só grava o que um retry com outro modelo/resolução poderia realmente resolver — o resto é índice desatualizado, e esconder isso no banco seria transformar um problema de descoberta em um problema de classificação.

### 5.11 O lote de 26/10 (925 arquivos) e por que ele pareceu 4× mais rápido

Segunda passada medida com o resumo novo, numa raiz recém-indexada:

| métrica | 25/10 (raiz cheia) | 26/10 (lote de 925) |
|---|---|---|
| wall | 5,84 s/arquivo | **1,47 s/arquivo** |
| requests/arquivo | 2,34 | **1,14** |
| p50 / p95 por request | 1,29 / 12,64 s | 1,28 / **1,47 s** |
| GPU / wall | 94 % | **98,4 %** |
| 500 / 400 | 186 / 19 | 16 / 0 |
| deduped / failed | 104 / 63 | 60 / 8 |

O `p50` por request é o mesmo. O que sumiu foi a **cauda de ~13 s**, que é o passe de OCR
dos documentos: essa pasta quase não tem documento (`max 13,37 s` aparece uma vez). Ou
seja, o ganho é de **composição do lote**, não de velocidade — e por isso a projeção para
una biblioteca mista continua sendo a de §5.8 (com ~15 % de documentos, +36 % de GPU).

Duas coisas que a medição confirmou de novo: 98,4 % do wall dentro da GPU (não há CPU
sobrando para esconder) e zero reloads/`KvSize` único com o agrupamento de §5.9.

**Encadeamento `full` → `backlog`.** `full` é o único modo que faz descoberta e limpeza;
o backlog é o que classifica com sobreposição. Agora um `full` que termina sozinho
dispara o passe de backlog da mesma raiz (`chain_backlog = true` em `settings.toml`), que
é também o único momento em que o produtor tem trabalho de CPU real para esconder do GPU.
Um scan cancelado **não** encadeia: cancelar significa parar, não continuar de outro jeito.

**Filtro de documento.** Uma foto de vôlei de areia com as placas de "BANCO DO BRASIL" e
"GOL" saiu como `[Doc: bank_transfer] [Issuer: GOL]`. A causa era o modelo responder
`is_document_like: true` para uma cena, e o portão aceitar isso. O schema primário agora
pede `"document_photo":"none|receipt|invoice|statement|other"` — a pergunta que o modelo
consegue responder ("a *foto* é esse documento?") — e o portão do OCR lê só esse campo:
uma cena responde `none` e não paga os ~13 s; um comprovante **fotografado** responde
`receipt` e volta a ser enriquecido, que era o recall que a regra anterior abria mão. Anime,
manga, artwork e vídeo continuam excluídos independentemente da resposta, e `document_kind`
mantém o `"none"` para o sufixo `[Doc]/[Issuer]` só entrar quando o texto do OCR tem sinal
de documento (valor + data, ou moeda).

O valor é persistido em `files.document_hint` (migration 16), com `''` = "não avaliado" e
`none` = "cena"; o bucket de documentos é `document_hint NOT IN ('', 'none')`
([`list_document_rows`](../sherlock/desktop/src-tauri/src/db.rs:3026)), que é a população
onde o A/B de OCR do §10 roda (LLM com 1500 tokens contra `num_predict` menor e contra o
surya).

**A/B de documentos (pendente).** Os 133 arquivos que disparavam o passe de 13 s em §5.8
ainda não foram medidos com os três braços; o bucket acima é onde eles aparecem.

### 5.12 `full` = descoberta + pipeline (implementado)

A descoberta escreve as linhas (`insert_pending_rows`, uma transação,
`ON CONFLICT DO NOTHING`, sem I/O além do walk). É o que impede o índice pela metade — um
scan cancelado agora deixa um índice completo com trabalho pendente, não um "925 de 50 000"
que parece concluído.

O corte entrou: **a fase 2 só roda no `refresh`** e o `full` caminha da descoberta direto
para o pipeline, no mesmo job. Antes de o pipeline começar, o scan carimba toda linha viva
(nova, modificada, inalterada, revivida) e zera o ponteiro morto das que perderam a
thumbnail; o `moved` é decidido pelo produtor, comparando o fingerprint da linha nova com
as linhas **vivas de outro marcador** (as que sumiram do disco), e o `merge_moved_row`
troca o caminho da linha antiga na mesma transação em que apaga o placeholder. O cleanup
(`mark_missing_as_deleted`) roda por último, e só quando o job não foi cancelado nem
abortado pelo breaker. `chain_backlog = false` mantém o formato antigo (fase 2 + laço de
classificação), como escape hatch.

O mapa do que era da fase 2 e para onde foi, e o que **não** pode regredir:

| o que a fase 2 faz | para onde vai |
|---|---|
| `fingerprint` (janela de 64 KB) | produtor, quando `files.fingerprint` está vazio |
| `location_text` (EXIF/GPS) | produtor, da mesma abertura do arquivo |
| `scan_marker` | produtor carimba toda linha que visita — **sem isso a limpeza do fim do scan trata a linha como sumida e faz soft-delete** |
| thumbnail + dhash | já é do produtor (é o objetivo do corte) |
| metadados de vídeo (ffprobe) | produtor, no ramo legado de vídeo |
| `moved` (fingerprint igual, caminho novo) | **o item mais delicado**: hoje é a fase 2 que casa o caminho novo com a linha antiga e move id + descrição. Sem isso, um arquivo renomeado vira linha nova pendente e a antiga é soft-deletada |

Os dois testes que o corte exigia existem em `scan.rs`:
`cancelled_full_scan_leaves_every_row_pending_and_alive` (cancelar pelo primeiro tick do
pipeline não soft-deleta nada e deixa as linhas não visitadas pendentes, sem thumbnail) e
`full_scan_keeps_unchanged_and_adopts_a_rename` (pasta mista: o inalterado fica com a
thumbnail e a descrição, o renomeado mantém id + descrição no caminho novo e a thumbnail é
reconstruída, o novo fica pendente, e o cleanup não apaga nenhum dos três). É o mesmo
terreno do bug de §5.7.6, onde a limpeza apagou uma pasta inteira.

Ganho esperado: os 58 743 arquivos deixam de pagar um passe mono-thread de thumbnails
(~4 h a 250 ms/arquivo) e passam a fazer thumb + classificação na mesma passada do
pipeline. **A medição na pasta de 191 arquivos ainda não foi feita** — é o próximo passo
antes de rodar nos 58 mil, com `scripts/measure-run.py 900` logo depois do job.

## 6. Riscos

| Risco | Mitigação |
|---|---|
| Contenção de CPU piora a latência por imagem | workers 2, reserva de 2 cores, gate A/B |
| Ler o mesmo arquivo 2× no SMB (se o produtor e o consumidor rodarem para linhas diferentes) | backlog combinado garante 1 read por arquivo |
| Cache de JPEG preparado cresce sem limite | default transiente; se persistente, apagar após classificar |
| Dedupe muda de representante | documentado; cluster e resultado continuam corretos |
| Qualidade: usar a imagem preparada em vez do original | `prepare` já é o que vai ao modelo hoje — bytes idênticos |
| Vídeo/PDF quebram com a mudança | fora do escopo da 1ª iteração |
| Trocar `num_ctx` entre requests faz o Ollama recarregar o modelo | contexto único por execução (`NUM_PREDICT_RESERVE` + `MIN_NUM_CTX = 4096`); medido: 316 reloads em 15 min antes da correção |
| Runner do Ollama morre por falta de VRAM e o job moí sem progresso | *circuit breaker* de 25 falhas consecutivas aborta com erro; recomendar `OLLAMA_FLASH_ATTENTION=1` e `OLLAMA_NUM_BATCH=128` |

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

## 8. Como medir a Fase 3 (o A/B)

Não compare **binários**. Três mudanças entraram juntas (filtro, decode
compartilhado, contexto fixo), então "app antigo vs app novo" mede a soma delas e
não decide nada sobre o pipeline. O binário novo contém **os dois algoritmos**:

| Braço | O que roda |
|---|---|
| **A** | `full`: a fase 2 mono-thread gera thumbnails, depois a fase 3 classifica sequencialmente |
| **B** | `backlog`: produtor (decode + thumbnail + JPEG do modelo) e consumidor (dedupe + visão) no mesmo passe |

Passos:

1. Reconstruir o app e aplicar o drop-in do Ollama (§5.5) — sem isso os dois
   braços sofrem com os crashes e a comparação vira ruído.
2. Escolher uma pasta com ~300 arquivos (mistura de fotos e algumas screenshots)
   na mesma raiz SMB. Anotar `N`.
3. Fixar as configurações nos dois braços: `qwen2.5vl:3b`, `image_max_side = 768`,
   `prompt_profile = concise`, `ocr_engine` igual nos dois, dedupe 6. Modelo já
   carregado (`ollama ps`) e nada mais usando a GPU.
4. **Braço A**: adicionar a pasta → `full` até terminar. Imediatamente rodar
   `scripts/measure-run.py N` e anotar o resultado.
5. Zerar o estado da pasta. **Remover a raiz na sidebar não limpa**: se a pasta
   está dentro de outra raiz indexada, [`remove_root`](../sherlock/desktop/src-tauri/src/lib.rs:441)
   transfere as linhas para a raiz pai (`reassign_to_parent_root`) e, ao adicionar
   a pasta de novo, [`adopt_child_files`](../sherlock/desktop/src-tauri/src/lib.rs:346)
   adota tudo de volta — com descrição, `confidence` e thumbnail. A varredura
   então encontra 188 de 191 já classificados e só processa os 3 sem descrição.
   O caminho normal agora é o menu da pasta → **Reclassify**, que tem duas formas:

   - **checkbox desmarcado** (padrão): limpa só a classificação, thumbnails
     ficam. É o estado do **E2** (só o pipeline), o mais rápido de rodar.
   - **checkbox marcado**: limpa classificação **e** thumbnails, reproduzindo a
     pasta recém-adicionada. É o estado do **E1** (ganho percebido), onde o
     produtor tem trabalho de CPU para esconder do GPU.

   Para apagar tudo de verdade (linhas, thumbnails, root, jobs) continua existindo
   [`scripts/reset-root.py`](../scripts/reset-root.py:1) — dry run por padrão,
   `--apply` para valer, com o app fechado. Depois de um reset por botão não é
   preciso remover/adicionar nada: o `full` regenera os thumbnails pelo caminho
   `heal` e classifica; o `backlog` vê as linhas com `thumb_path IS NULL`.
6. **Braço B**: `Process Backlog` até terminar → mesmo `scripts/measure-run.py N`.
7. Preencher a tabela e aplicar o critério.

O script não faz parte do pipeline: é só o termômetro. Rode-o logo depois de cada
braço e **antes** de começar o outro, porque ele resume *um* job (o último, ou o
`--job N` que você indicar) e não separa duas execuções misturadas.

O que o [`scripts/measure-run.py`](../scripts/measure-run.py:1) tira do índice +
journal, sem precisar anotar nada na mão:

- o job: modo, status, arquivos, wall total, média por arquivo;
- **quantas linhas daquela raiz ainda estão pendentes** (sem descrição) — a
  conferência honesta de "terminou mesmo?", já que `confidence` é do modelo (§5.6);
- requisições `/api/generate`, duração p50/p95/máx e o tempo total de GPU;
- `model loads`, `runner spawns`, `runner crashes`, e os `KvSize`/`BatchSize`
  usados — mais de um `KvSize` significa runner sendo reconstruído (§5.5), e
  qualquer crash invalida o braço, porque a requisição se perdeu.

Antes de comparar os braços, confirme no resumo de A e de B: `runner crashes: 0` e
`model loads: 1`. Se não, o número mede o Ollama se recuperando, não o pipeline.

### 8.1 Resultado medido (191 arquivos, `2020`, Ollama 0.40.11)

| braço | E1 (thumbs do zero) | E2 (thumbs prontos) |
|---|---|---|
| **A** `full` | 511 s (2,68 s/arq) | 489 s (2,56 s/arq) |
| **B** `backlog` | **423 s** (2,21 s/arq) | **420 s** (2,20 s/arq) |
| B/A | **0,83** (−17%) | **0,86** (−14%) |
| p95 por request | 3,67 → **3,55** s | 3,83 → **3,61** s |
| requests | 302 → 303 | 305 → 303 |

O pipeline **nunca é mais lento e melhora o p95 nas duas comparações**, mas o ganho
fica em 14–17%, abaixo do gate de 20%. Três coisas explicam o teto:

1. **A GPU domina**: ~420 s dos ~490 s são requisições. Sobrepor CPU só rende o que
   existe de CPU, e decode+prepare somam ~60–70 s nessa pasta.
2. **Thumbnail é barato aqui**: 191 JPEGs pequenos custam ~22 s (511 vs 489 s). Por
   isso E1 e E2 quase empatam — era a previsão: nessa pasta o E1 não separa os braços.
3. **8–9 arquivos entram em loop** de repetição e gastam 4 requisições cada (~30 de
   300 = 10% do tempo) nas duas pontas.

Veredito: **liga**, com a ressalva de que o gate de 20% não foi atingido e de que o
próximo ganho está nos arquivos que entram em loop, não no pipeline.

Critério de decisão (o mesmo do §5):

| Resultado | Decisão |
|---|---|
| B ≤ 0,80 × A em wall total **e** p95 de B não pior | **liga**: o pipeline passa a ser usado também no `full` |
| B ≥ A, ou p95 de B pior | **desliga**: fica só o decode compartilhado + filtro, que já valem por si |

Duas leituras possíveis, escolha uma:

- **Ganho percebido** (A = `full`): inclui o tempo em que a GPU fica ociosa
  durante a fase 2. É o número que o usuário sente, mas mistura a descoberta.
- **Só classificação** (mais limpo): rodar `refresh` antes dos dois braços e então
  A = `classify_only`, B = `backlog`. O índice fica idêntico (thumbnails
  presentes, nada classificado) e a diferença é exatamente o pipeline.

## 9. Perguntas abertas

1. Cache de JPEG preparado: **decidido transiente** nesta fase. Persistente
   (para o resume de `classify_only` não pagar decode) fica para depois de a
   Fase 2 mostrar quanto tempo o pipeline realmente economiza.
2. Vale a pena um botão "Processar backlog" ou o pipeline entra só no `full`?
3. Vídeo (1 vs 5 keyframes) é a próxima iteração ou fica fora de escopo?
4. Filtro do resize: **decidido** — `auto` usa box para JPEG/HEIC e Lanczos3 para
   PNG/BMP/TIFF. Falta apenas a validação de qualidade em screenshots
   (`BENCH_QUALITY=1`) antes de considerar o padrão definitivo.
5. Botão "Reclassificar esta pasta" na sidebar: hoje o único jeito de zerar um
   recorte é o [`scripts/reset-root.py`](../scripts/reset-root.py:1), e isso é
   surpresa ruim para quem clica em "remover" esperando exatamente isso (§8, passo
   5). Precisa decidir entre reclassificar (limpar só as colunas de classificação)
   e purgar (apagar linhas e thumbnails).
