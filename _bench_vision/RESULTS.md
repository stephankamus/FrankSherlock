# Vision classification speed — local benchmark

Hardware: RTX 3060 12 GB, 32 GB RAM, Ollama 0.23.1, driver 580.178.04.
Corpus: `~/Imagens/Festa de Formatura` (26 JPEGs, 1280×1280 / 1280×960, ~180 KB each).
Modelos: `qwen2.5vl:7b` (Q4_K_M), `qwen2.5vl:3b` (baixado durante o teste).

Scripts: [`bench_vision.py`](bench_vision.py), [`bench_ctx.py`](bench_ctx.py),
[`bench_compare.py`](bench_compare.py).

`wall` = tempo observado pelo cliente. `gap` = `wall − prefill − decode` (pré-processamento
de imagem + transferência HTTP + serialização, tudo **fora** da GPU).
Projeção = `wall × 53.563 / 3600`.

## 1. Baseline atual do app (prompt de `classify.rs`, imagem original, `num_ctx` default)

| Config | wall | prefill | decode | decode t/s | VRAM / total | Projeção 53.563 |
|---|---|---|---|---|---|---|
| 7b, original 1280px, PRIMARY_PROMPT, npred=500, ctx=16384 | **20,11 s** | 4,58 s (2302 tok) | 8,66 s (78 tok) | **9,1** | 11,54 / 17,05 GB (**5,5 GB em RAM**) | **299 h** |

`ollama ps` no baseline: `size=17,05 GB`, `size_vram=11,54 GB`, `context_length=16384`.
Ou seja: **~1/3 do modelo roda em CPU**. Isso é confirmado por um teste sem imagem
(texto puro, 120 tokens): **8,7 t/s** — uma 3060 faz 35–45 t/s com 7B Q4 inteiro na VRAM.

## 2. O que cada mudança isolada resolve

| Config | wall | prefill | decode | decode t/s | spill p/ RAM | Projeção |
|---|---|---|---|---|---|---|
| 7b, 1024px, PRIMARY, **ctx=4096** | 10,25 s | 2,05 s (1555) | 4,26 s (78) | 18,3 | 3,0 GB | 152 h |
| 7b, 768px, PRIMARY, ctx=4096 | 6,68 s | 0,92 s (915) | 4,18 s (78) | 18,7 | 3,0 GB | 99 h |
| 3b, 1024px, PRIMARY, ctx default | 6,23 s | 1,21 s (1555) | 1,33 s (82) | **62,0** | 1,8 GB | 93 h |
| 7b, 768px, **SHORT_PROMPT**, npred=180, ctx=4096 | 3,44 s | 0,65 s (814) | 1,56 s (30) | 19,3 | 3,0 GB | **51 h** |
| 3b, 1024px, SHORT_PROMPT, npred=180 | 3,73 s | 0,39 s (1454) | 0,51 s (32) | 63,8 | 1,8 GB | 55 h |
| 3b, 768px, SHORT_PROMPT, npred=180 | 2,30 s | 0,26 s (814) | 0,57 s (38) | 63,9 | 1,8 GB | **34 h** |
| 3b, 640px, SHORT_PROMPT, npred=180 | 1,76 s | 0,20 s (614) | 0,46 s (31) | 63,7 | 1,8 GB | **26 h** |

`SHORT_PROMPT` = schema reduzido a `media_type` + `contains_text` + `description` (≤12 palavras)
+ `confidence`, sem `series_candidates` / `character_candidates` / `is_*_related` na 1ª passada.
JSON válido em **100%** das respostas (formato `json` do Ollama, zero retries).

### Conclusões dos números

1. **`num_ctx` é o maior ganho isolado no 7b**: 16384 → 4096 dobra o decode (9,1 → 18,3 t/s)
   e remove 2,5 GB de spill. Causa: com 16k de contexto o working set (17,05 GB) não cabe
   nos 12 GB da 3060.
2. **O 7b nunca cabe inteiro na 3060** (sobra 3 GB em RAM mesmo em ctx=4096).
   O 3b cabe e roda a **~64 t/s** (7× mais rápido que o 7b no estado atual).
3. **Tamanho da saída pesa mais que o tamanho da imagem**: 78 → 30 tokens no 7b corta
   48% do wall time (6,68 → 3,44 s), porque decode é o gargalo.
4. **Resolução importa pela via indireta**: o `gap` (pré-processamento de imagem do Ollama,
   CPU) cai de 2,82 s (1024px) para 1,48 s (768px) e 1,10 s (640px). A partir de ~1280px o
   Ollama já limita os tokens de imagem (2302 a 1280px vs 2782 a 4000px), então subir
   resolução só aumenta o `gap`.
5. **Concorrência quase não ajuda**: 4 workers deram 3,53 s/img efetivo vs 3,73 s sequencial.
   O Ollama serializa (`OLLAMA_NUM_PARALLEL=1`) e o decode é limitado por banda de memória.

## 3. Configuração recomendada

### Opção A — manter 7b (melhor qualidade), só otimizar

`num_ctx=4096` + downscale 768px + prompt curto/npred 180 → **~51 h** (era ~150–300 h).
Ganho: ~4–6×, sem trocar de modelo.

### Opção B — 3b na passada principal

`num_ctx=4096` + 768px + prompt curto → **~34 h**; com 640px → **~26 h**.
Qualidade precisa ser validada com um conjunto de anime/screenshot (as 26 fotos de formatura
são todas `photo`, classe trivial — não discriminam 3b de 7b).

### Opção C — híbrido (recomendado)

3b a 768px na passada principal (34 h de base) e 7b **apenas** quando
`is_anime_related`/`media_type == anime|manga` exigir identificação de série.
Se 15% da biblioteca cair nesse caso: ~34 h + 0,15 × 51 h ≈ **41 h**.

## 3.1 Fotos grandes reais (HEIC 2252×4000, NEF 4644×3084)

`~/Imagens/Fotos Grandes` tem 16 HEIC, 8 NEF e 3 MP4 — **nenhum HEIC/NEF entra no índice**
(`SUPPORTED_EXTS` em `scan.rs` não tem `.heic`/`.nef` e o crate `image` não decodifica nenhum
dos dois). Para medir, os arquivos foram convertidos em resolução cheia num venv isolado
(`pillow-heif` + `rawpy`) por [`bench_big.py`](bench_big.py).

> **Resolvido depois desta medição**, e não pelo pipeline: existe agora um fluxo opt-in que
> converte HEIC/NEF em cópias JPEG ao lado dos originais
> ([`plans/heic-nef-conversion.md`](../plans/heic-nef-conversion.md)). Medido com o sidecar
> novo nos mesmos arquivos: HEIC 3,5 MB em 310 ms; NEF usando o **preview embutido de
> 4608×3072** em 107 ms (o demosaico completo custa 629 ms). O que segue abaixo continua
> válido como medição do pipeline de visão sobre imagens já convertidas.

| Config | wall/foto | prefill | decode | gap (`wall − prefill − decode`) |
|---|---|---|---|---|
| 7b, original 9–14 MP, PRIMARY, ctx 16384 | **24,31 s** | 6,11 s (2801 tok) | 8,08 s (71) | **10,12 s** |
| 7b, 1024px, PRIMARY, ctx 4096 | 7,80 s | 1,35 s (1018) | 3,88 s (72) | 2,57 s |
| 3b, 1024px, SHORT | 3,45 s | 0,66 s (918) | 0,45 s (29) | 2,34 s |
| 3b, 768px, SHORT | 2,48 s | 0,18 s (530) | 0,46 s (30) | 1,84 s |
| 3b, 640px, SHORT | 2,18 s | 0,13 s (407) | 0,45 s (29) | 1,59 s |

Em fotos de 9–14 MP o `gap` do Ollama (decodificação + resize + tokenização) chega a
**10,12 s** — sozinho, mais que o dobro do tempo total das configs otimizadas.

### `num_ctx` dimensionado pela imagem zera o spill

| Config | wall | VRAM / total | spill |
|---|---|---|---|
| 3b, 768px, SHORT, ctx 2048 | 2,09 s | 11,40 / 11,40 GB | **0,00 GB** |
| 3b, 1024px, SHORT, ctx 4096 | 2,74 s | 11,65 / 11,65 GB | **0,00 GB** |
| 3b, 1024px, SHORT, ctx 2048 | 2,67 s | 11,40 / 11,40 GB | **0,00 GB** |

O `spill` de 1,8 GB visto antes era só o contexto de 16384 tokens:
com `num_ctx` derivado do tamanho da imagem o 3b passa a rodar **inteiro na GPU**.

### Qualidade x resolução (3b, mesma foto)

| Resolução | Descrição gerada (burro na estrada de terra) |
|---|---|
| 1024px | *"Donkey lying on dirt road"* ✔ |
| 768px | *"A brown animal lying on a dirt path…"* ~ |
| 640px | *"Elephant lying on dirt road"* ✘ |

768px é o piso aceitável; 640px começa a errar o sujeito.

## 3.3 Pós-implementação (medido pelo próprio app)

O teste de fumaça do pipeline real (`prepare → Ollama → parse`) roda com as novas
configurações padrão (1024 px, perfil conciso, `num_ctx` derivado da imagem):

```sh
FRANK_VISION_SMOKE_DIR="$HOME/Imagens/Festa de Formatura" \
FRANK_VISION_SMOKE_MODEL=qwen2.5vl:3b \
  cargo test vision_pipeline_smoke -- --ignored --nocapture
```

| Medida (modelo quente) | Antes | Depois |
|---|---|---|
| 1 imagem, ponta a ponta | 20,11 s | **3,09 s** |

A primeira execução inclui o carregamento do modelo (9,07 s); as seguintes ficam em ~3,1 s.
A qualidade se mantém: `media_type="photo"`, `confidence=0.95`,
*"Man in formal attire standing next to a cake and cupcakes."*

## 4. Ainda não medido (mas com forte suspeita)

`run_surya_ocr()` (`classify.rs`) sobe um processo Python por imagem
(`process::Command`) — o benchmark do repo mede **8,15 s/imagem** para o Surya contra
1,73 s do LLM de visão. Esta pasta não tem screenshots/documentos, então o custo não
apareceu aqui. Em bibliotecas com texto essa é a maior alavanca, e ela é maior que
qualquer ganho de modelo: **gate do OCR** é obrigatório.

## 5. Reproduzir

```bash
# baseline vs downscale vs prompt curto vs 4000px
python3 -u _bench_vision/bench_vision.py --dir "$HOME/Imagens/Festa de Formatura" --limit 10

# 7b vs 3b
python3 -u _bench_vision/bench_compare.py

# varredura de resolução + concorrência (3b)
# (ver histórico do bench_ctx.py / seção 2)
```

## 6. Notas de medição

- Uma rodada de `bench_ctx.py` produziu números inválidos (`vram=0.00 GB`, prefill 99 s):
  durante ela o `nvidia-smi` passou a falhar com `Failed to initialize NVML:
  Driver/library version mismatch` e o Ollama rodou 100% em CPU. O driver voltou ao normal
  (580.178.04) na rodada seguinte; os números das tabelas acima foram todos obtidos com
  GPU saudável e `size_vram > 0`.
- `load_duration` foi excluído das médias (modelo pré-aquecido); carregar o 7b do disco
  frio levou 50 s.

## 7. Pipeline CPU/GPU — Fase 0 (medir antes de construir)

Plano: [`plans/pipeline-cpu-gpu-overlap.md`](../plans/pipeline-cpu-gpu-overlap.md).
Mede a duplicação atual — a thumbnail e `vision_prep::prepare` decodificam o **mesmo**
arquivo duas vezes — e quanto custaria eliminá-la.

```bash
cd sherlock/desktop/src-tauri
BENCH_ROOT=/caminho/da/raiz BENCH_LIMIT=200 BENCH_MODEL=qwen2.5vl:3b \
  cargo test --release bench_cost_split -- --ignored --nocapture
```

Colunas por arquivo (ms): `read` (primeiro toque — o custo de I/O que o cache local não
esconde), `decode` (decompressão pura, sem I/O), `dhash`, `thumb` (total), `prepare`
(total), `request`, `prefill`, `decode_gpu` e `ollama_cpu` (`request − prefill − decode`,
tudo em CPU dentro do Ollama).

A unidade duplicada é **`read + decode`**: é o que o pipeline remove por arquivo.
Registrar hardware, modelo, `image_max_side` e se a raiz é SMB ou disco local — o número
não é transportável entre máquinas.

| Data | Raiz | mídia | modelo | max_side | read | decode | thumb | prepare | request | ollama_cpu | duplicado / seq | veredito |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2026-10-25 | `smb-share:...share=fotos/…/Baixa dos 2020` (GVFS) | SMB | — | 1024 | 22,1 | 10,2 | 76,6 | **195,4** | — | — | 32,3 / 272,0 (12 %) | só CPU; medir com modelo |
| 2026-10-25 | `smb-share:...share=fotos/Fotos/2026/Ilha do Mangabal` (GVFS) | SMB | qwen2.5vl:3b | 1024 | 136,3 | 50,7 | 300,3 | **606,7** | 2105,6 | 1519,6 | 187,1 / 3012,6 (**6 %**) | teto do pipeline **26 %** |
| — | — | — | — | — | — | — | — | — | — | — | — | pendente |

Segunda rodada (174 arquivos, 1,41 MB de média, 16 vCPU, sem o app rodando):

- **A ordem dos ganhos se inverteu.** O duplicado é 6 % do custo, mas o request tem
  **1519,6 ms de CPU com a GPU ociosa** (prefill+decode = 586 ms). Adiantar thumb+prepare
  nessa janela vale até **26 %** no total. O critério "duplicado ≥ 20 %" era o critério
  errado: ele ignora a sobreposição, que é o objetivo da mudança.
- **Viabilidade**: produtor = 907 ms (thumb+prepare) contra consumidor = 2105,6 ms por
  arquivo. Um produtor entrega 2,3× mais rápido do que o consumidor consome. Não é cenário
  de contenção.
- **`prepare` = 20 % do total** (606,7 ms). É mais que o decode compartilhado e não depende
  de concorrência: o filtro box da thumbnail custa uma fração do Lanczos3.
- `image::open` (154,3) é 18 % **mais barato** que `read + load_from_memory` (187,1) neste
  mount. O decode compartilhado deve decodificar via `image::open` uma vez e reusar o
  `DynamicImage`, não "ler para memória".
- `dhash` custa 81,3 ms sobre a imagem em resolução cheia.
- **NEF não é lido**: os arquivos de 14 MB ficaram fora (`SUPPORTED_EXTS` não tem `.nef` e o
  `image` não decodifica NEF). Com NEF no índice, o duplicado subiria para ~2,4 s/arquivo.

### 7.1 Fase 0.5 — filtro do downscale (A/B, 6 arquivos, sem modelo)

| Kernel | ms/arquivo |
|---|---|
| box (`imageops::thumbnail`) | **383,8** |
| Lanczos3 (anterior) | 571,4 |
| diferença | **−187,6 (−33 %)** |

−6,2 % do custo total por arquivo medido acima, sem concorrência. Implementado como
`settings.toml: image_filter = "auto" | "box" | "lanczos"`; `auto` usa box para JPEG/HEIC e
Lanczos3 para PNG/BMP/TIFF.

Pendente: `BENCH_QUALITY=1` (exige `BENCH_MODEL`) imprime as respostas dos dois kernels lado
a lado — rodar em uma pasta de screenshots/texto fino antes de confiar no `auto` como padrão
para documentos.

### 7.2 Fase 1 — um decode, dois artefatos (6 arquivos, sem modelo)

| Caminho | ms/arquivo |
|---|---|
| `generate_thumbnail` + `prepare` (2 decodes, hoje) | 1061,9 |
| `decode_source` + `thumbnail_from_decoded` + `prepare_from_decoded` | **578,0** |
| diferença | **−483,9 (−46 %)** |

O caminho compartilhado elimina um read, um decode, um `extract_orientation` (que abre o
arquivo de novo) e um `stat` por arquivo.

Ressalva de medição: esta rodada saiu ~1,5× mais lenta que a das 02:58 na mesma pasta
(`thumb` 427 vs 280; `prepare box` 635 vs 384; load average 4). A causa é variabilidade do
NAS/SMB, não do código — por isso só o **delta dentro da mesma rodada** é confiável, e o
efeito no total fica entre −10 % e −16 % do custo por arquivo de §7.

Primeira rodada (3 arquivos, 0,24 MB cada, JPEG de WhatsApp):

- `image::open` = 30,6 ms ≈ `read + decode` (95 %) — o `BufReader` do `image` **não** é um
  problema no GVFS; a estratégia de leitura não precisa mudar.
- `prepare` (195,4 ms) custa **2,5×** a thumbnail (76,6 ms) e **6×** o read+decode. O custo
  não está no I/O nem no decode: é Lanczos3 + `to_rgb8` + encode q88 + o `stat`/EXIF extra
  que o `prepare` faz por conta própria. A thumbnail usa o filtro box de `imageops::thumbnail`,
  que é muito mais barato.
- Sem `BENCH_MODEL` os percentuais são relativos só à CPU. Com o request de ~3,5 s (3b a
  1024 px) o duplicado de 32 ms vira **~1 %** do custo por arquivo nesta classe de tamanho.
- Conclusão preliminar: para JPEGs pequenos o pipeline não paga; o ganho mora nos arquivos
  grandes (12–14 MP) e na **paralelização** da fase 2, que hoje usa 1 core.

## 8. Idioma da descrição — PT-BR vs inglês (2026-10-27)

Script: `scripts/bench-description-language.py` (12 fotos reais do índice, `qwen2.5vl:3b`,
`image_max_side = 768`, `num_ctx = 4096`, `format: json`, temperatura 0, ordem das chamadas
alternada para cancelar o cache de imagem do Ollama).

| Variante | tokens de saída | decode | observação |
|---|---|---|---|
| EN (prompt atual) | 46,2 | 0,38 s | |
| PT-BR, limite de 12 palavras | **43,2** | **0,35 s** | 12/12 JSON válido |
| PT-BR, limite de 16 palavras | 61,2 | 0,50 s | 1/12 entrou em loop até o teto de 180 tokens |
| EN + request de tradução | 46,2 + 15 | 0,38 + 0,15 s | **+18 a 23 %** por imagem |

Conclusões:

- **O qwen2.5-VL escreve PT-BR nativamente e o custo é nulo** (menos tokens que em inglês,
  porque diz o mesmo em menos palavras). O delta é ~0,03 s/imagem: ~30 min em 58,7 mil
  arquivos, ~2 % de uma rodada de 22 h.
- **Não vale traduzir depois**: um request extra por arquivo (+18–23 %) e a tradução do 3b
  erra (medido: "Two men lighting a candle" → `Dois pessoas acendendo uma vela`;
  "Man surfing on a purple board" → `Praticando surf de man oceano`).
- **O limite de 12 palavras não pode subir**: com 16 o modelo entrou em repetição
  (`Pessoa de pé em pé de pé de pé…`), que é o `token repeat limit` já documentado em
  `classify.rs`.
- Qualidade: PT-BR no 3b é mais **telegráfico** e às vezes pior que o inglês
  (`Girl blowing out candle on birthday cake` → `jovem segurando algo invisível`). Por isso
  as linhas antigas são convertidas por **tradução** (que herda a análise boa em inglês) e
  não por uma nova passada de visão.

Medição da conversão das linhas existentes (`descriptions.rs`, texto puro, sem imagem):
0,09–0,14 s por linha em regime, ~8 min para as 3.664 linhas descritas do índice atual.

Duas armadilhas medidas na conversão:

- placeholder (`⟦0⟧`) é **descartado** pelo modelo: 7 de 8 linhas voltaram sem ele;
- mandar o marcador no prompt faz o modelo **traduzi-lo** (`[Doc: bank_transfer]` →
  `[Documento: transferência bancária]`), o que quebra `document_kind_of` e o bucket de
  documentos.

Solução: os trechos entre colchetes **não são enviados** (`split_markers`), a prosa é
traduzida e os bytes originais dos marcadores são recolocados (`join_markers`) — 10 de 10
linhas preservaram todos os marcadores.

Busca: verificado contra o tokenizer real (`porter unicode61`) — acentos são dobrados nos
dois sentidos (`coração` = `coracao`), plurais casam (`casamento` = `casamentos`) e prefixo
funciona (`anivers*`, `crian*`). Só flexão verbal não casa (`nadar` ≠ `nadando`, `nada*` casa).
