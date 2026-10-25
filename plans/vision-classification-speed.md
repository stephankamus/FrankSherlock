# Plano — velocidade e seleção de modelo na classificação por visão

> **Status: implementado.** Passos 1–5 da ordem abaixo estão no código
> (`vision_prep.rs`, `vision_estimate.rs`, `llm/client.rs`, `classify.rs`,
> `config.rs`, `ModelPickerModal.tsx`, `similarity::DhashIndex`). Medido de ponta
> a ponta pelo teste `vision_pipeline_smoke`: 20,11 s → **3,09 s** por imagem.
> Dedupe por dhash (passo 5) entregue em
> [`thumbnail-cache-and-dhash-dedupe.md`](./thumbnail-cache-and-dhash-dedupe.md):
> fotos com Hamming ≤ `dedupe_dhash_distance` (padrão 6) herdam a classificação
> do representante e thumbnails byte-idênticos não são decodificados de novo.
> HEIC/NEF (passo 6) continua pendente.

Medições completas em [`_bench_vision/RESULTS.md`](../_bench_vision/RESULTS.md).
Hardware de referência dos testes: RTX 3060 12 GB / 32 GB RAM / Ollama 0.23.1.
**Nenhum número desta máquina pode virar constante no código** — o desenho abaixo é
derivado, com fallback para GPU desconhecida / CPU.

---

## 1. Evidência (o que os testes mostraram)

### 1.1 Fotos comuns (1280 px, 26 arquivos)

| Config | wall/foto | prefill | decode | t/s | spill p/ RAM |
|---|---|---|---|---|---|
| 7b, original, prompt atual, `num_ctx` default (16384) | 20,11 s | 4,58 s | 8,66 s | **9,1** | 5,5 GB |
| 7b, 1024 px, prompt atual, ctx 4096 | 10,25 s | 2,05 s | 4,26 s | 18,3 | 3,0 GB |
| 3b, 1024 px, prompt atual, ctx 16384 | 6,23 s | 1,21 s | 1,33 s | 62,0 | 1,8 GB |
| 7b, 768 px, prompt curto | 3,44 s | 0,65 s | 1,56 s | 19,3 | 3,0 GB |
| 3b, 1024 px, prompt curto | 3,73 s | 0,39 s | 0,51 s | 63,8 | — |
| 3b, 768 px, prompt curto, **ctx 2048** | **2,09 s** | 0,18 s | 0,27 s | 64 | **0,0 GB** |
| 3b, 640 px, prompt curto | 1,76 s | 0,20 s | 0,46 s | 63,7 | — |

### 1.2 Fotos grandes reais (HEIC 2252×4000, NEF 4644×3084)

| Config | wall/foto | prefill | decode | gap (pré-processamento) |
|---|---|---|---|---|
| 7b, original 9–14 MP, prompt atual, ctx 16384 | 24,31 s | 6,11 s (2801 tok) | 8,08 s | **10,12 s** |
| 7b, 1024 px, prompt atual, ctx 4096 | 7,80 s | 1,35 s | 3,88 s | 2,57 s |
| 3b, 1024 px, prompt curto | 3,45 s | 0,66 s | 0,45 s | 2,34 s |
| 3b, 768 px, prompt curto, ctx 2048 | 2,48 s → 2,09 s | 0,18 s | 0,46 s | 1,84 s |
| 3b, 640 px, prompt curto | 2,18 s | 0,13 s | 0,45 s | 1,59 s |

Conclusões que sustentam o desenho:

1. **`num_ctx` default (16384) é o maior desperdício**: aloca KV para 16k tokens que nunca são
   usados (a imagem consome ~1500 tok a 1024 px). Reduzir para 2048–4096 **dobra** o decode do
   7b e faz o **3b caber 100% na VRAM** (`spill = 0,00 GB`).
2. **Resolução pesa pelo `gap`, não pelo prefill**: `wall − prefill − decode` vai de 10,12 s
   (9 MP original) para 2,57 s (1024 px) e 1,59 s (640 px). É decodificação + resize +
   tokenização de imagem do Ollama, em CPU.
3. **Tamanho da saída pesa mais que o tamanho da imagem no 7b**: 78 → 30 tokens corta 48 % do
   wall time.
4. **Concorrência não ajuda** (4 workers = 3,53 s/img vs 3,73 s sequencial). Ollama serializa.
5. **Qualidade cai abaixo de 768 px**: a 640 px o 3b descreveu um burro como *"elephant"* e
   *"brown animal"*; a 1024 px acertou *"donkey"*. 768 é o piso aceitável, 1024 o padrão seguro.

---

## 2. Decisões

| Item | Decisão | Por quê |
|---|---|---|
| Downscale antes do base64 | **Implementar** | `gap` 10,12 s → 2,57 s em fotos grandes |
| `num_ctx` derivado da imagem | **Implementar** | 2× decode no 7b; 3b passa a caber inteiro na VRAM |
| Prompt curto + `format: json` | **Implementar** | 48 % do tempo no 7b; 100 % de JSON válido; remove as 3 tentativas |
| Gate do OCR (Surya) | **Implementar** | 8,15 s/imagem no benchmark do repo; dispara hoje em qualquer `contains_text` |
| Não reclassificar duplicadas (dhash) | **Implementar, medindo primeiro** | dhash já existe; ganho depende da biblioteca |
| 2–3 workers | **Descartar** | medido: ganho ≈ 0 % |
| Seleção de modelo em modal | **Implementar** | 3b vs 7b depende da VRAM e do conteúdo da biblioteca |
| HEIC / NEF | **PR separado** | hoje nem entram no índice (`SUPPORTED_EXTS`) |

---

## 3. Desenho

### 3.1 Normalização de imagem (novo módulo)

Novo `sherlock/desktop/src-tauri/src/vision_prep.rs`:

```rust
pub struct PreparedImage {
    pub path: PathBuf,      // arquivo temporário (JPEG) quando houve resize
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

/// Reduz para `max_side` e reencoda em JPEG q85 quando necessário.
/// Retorna o próprio caminho quando a imagem já cabe (evita reencode inútil).
pub fn prepare(source: &Path, max_side: u32, tmp_dir: &Path) -> AppResult<PreparedImage>
```

Pontos de integração:
- [`classify.rs`](../sherlock/desktop/src-tauri/src/classify.rs:46) — `classify_primary`,
  `classify_anime_details`, `run_llm_ocr` recebem `&PreparedImage` em vez de `&Path`.
- Um `prepare` por arquivo, **reutilizado** por todas as chamadas (hoje a imagem é relida e
  reencodada em cada tentativa).
- `max_side` configurável (`VisionSettings`, ver 3.5), default **1024**, mínimo 640.
- Regra prática: arquivos com `max(w,h) <= max_side` não são reencodados (só passam o caminho);
  JPEGs grandes e PNG/BMP/TIFF são reencodados.

### 3.2 `num_ctx` derivado da imagem (não da VRAM)

**Por que não calcular a partir da VRAM:** o `num_ctx` só precisa cobrir *uma* imagem por
requisição. Depois do downscale para 1024 px o teto é ~1600 tokens, então 4096 é
suficiente em qualquer GPU. Calcular por VRAM exigiria ler `model_info` do `/api/show`
(`block_count`, `head_count_kv`, `embedding_length`), somar `mmproj` e reservas do driver —
frágil entre versões do Ollama e entre backends (CUDA/ROCm/Metal/CPU). O que a VRAM determina
de verdade é **qual modelo dá para usar**, e isso pertence ao seletor (3.6).

```rust
/// Estimativa conservadora de tokens de imagem do qwen2.5-VL.
fn image_tokens(w: u32, h: u32) -> u32 {
    let patches = w.div_ceil(28) * h.div_ceil(28);
    (patches as f32 * 1.2) as u32 + 128
}

pub fn num_ctx_for(img_w: u32, img_h: u32, prompt_chars: usize, num_predict: u32) -> u32 {
    let needed = image_tokens(img_w, img_h)
        + (prompt_chars / 4) as u32   // ~4 chars por token
        + num_predict
        + 256;                        // margem
    needed.next_power_of_two().clamp(2048, 8192)
}
```

Validação contra o medido:

| max_side | tokens de imagem medidos | `num_ctx` resultante |
|---|---|---|
| 640 | 407–614 | 2048 |
| 768 | 530–814 | 2048 |
| 1024 | 918–1555 | 4096 |
| 1280 | 2302 | 4096 |

Enviar **sempre** `options.num_ctx` (hoje [`client.rs`](../sherlock/desktop/src-tauri/src/llm/client.rs:31)
não envia e o Ollama usa 16384). `num_ctx` deve ser parâmetro de `ollama_generate`.

### 3.3 Prompt curto + JSON estruturado

- 1ª passada passa a pedir só o que decide o roteamento e a busca:
  `media_type`, `contains_text`, `description` (≤12 palavras), `confidence`.
  `is_anime_related` / `is_document_like` saem do schema e passam a ser derivados de
  `media_type` ([`should_run_anime_enrichment`](../sherlock/desktop/src-tauri/src/classify.rs:219)
  já aceita `media_type` puro); `series_candidates` / `character_candidates` ficam **apenas**
  na passada de anime ([`ANIME_PROMPT`](../sherlock/desktop/src-tauri/src/classify.rs:47)).
- `num_predict`: 500 → **180** na 1ª passada, 400 no anime, 300 em documentos.
  Cap de OCR text: 2000 → 1200 caracteres para extração de campos.
- `format: json` **sempre** (hoje só na tentativa 1). Com JSON válido em 100 % dos testes,
  [`classify_primary`](../sherlock/desktop/src-tauri/src/classify.rs:359) cai de 3 tentativas
  para **1 + 1 retry sem `format`** apenas em caso de erro HTTP/parse.
- `temperature: 0` na classificação (determinismo entre execuções).

### 3.4 Gate do OCR

Hoje [`should_run_document_enrichment`](../sherlock/desktop/src-tauri/src/classify.rs:231)
dispara com `contains_text == true` — anime com legenda queimada entra no Surya (8 s).
Novo critério:

```rust
pub fn should_run_document_enrichment(primary: &Value) -> bool {
    let media_type = ...;
    if matches!(media_type, "anime" | "manga" | "artwork") { return false; }
    let is_doc = ...;
    matches!(media_type, "document" | "screenshot") || is_doc
}
```

`contains_text` continua sendo gravado (útil na busca), só deixa de decidir o OCR.
Adicionar `VisionSettings::ocr_engine: Off | Llm | Surya` (default `Llm`, que é 1,73 s vs 8,15 s),
para o usuário escolher recall x velocidade.

### 3.5 `VisionSettings` (config, não hardcode)

Em [`config.rs`](../sherlock/desktop/src-tauri/src/config.rs:154) (`settings.toml` já existe
para `model_override`):

```toml
model_override = ""         # vazio = recomendação automática pela GPU
image_max_side = 1024       # 0 = original | 1024 | 768
prompt_profile = "concise"  # concise | detailed
ocr_engine = "llm"          # off | llm | surya
```

O modal expõe quatro escolhas, cada uma com o impacto medido no rótulo:

**Modelo** — lista de `/api/tags` com badge de encaixe na VRAM
(ex.: *"7b — 6,0 GB; não cabe em 12 GB → ~3 GB em RAM, ~2× mais lento"*).

**Resolução** (`image_max_side`):

| Opção | Custo medido (foto 12–14 MP) | Impacto |
|---|---|---|
| Original | `gap` 10,12 s + 2801 tok | Sem reencode; melhor para texto fino; **~7× mais lento** |
| **1024 px** (padrão) | `gap` 2,57 s + ~1000 tok | Mantém sujeitos pequenos e texto legível |
| 768 px | `gap` 1,84 s + ~530 tok | 25–40 % mais rápido; começa a perder sujeitos pequenos |
| 640 px | `gap` 1,59 s + ~407 tok | Erra o sujeito (*burro* → *elefante*): não oferecer |

**Perfil de prompt** (`prompt_profile`):

| Opção | Saída | Impacto |
|---|---|---|
| **Concisa** (padrão) | ~29–34 tok | `media_type` + `contains_text` + descrição ≤12 palavras |
| Detalhada | ~71–82 tok | Schema atual com candidatos de série/personagem; **+48 % no 7b**, +5 % no 3b |

**OCR** (`ocr_engine`): `Off` / `Llm` (padrão, 1,73 s/img) / `Surya` (8,15 s/img, melhor recall).

Fluxo: `list_vision_models()` (nova command) → modal → grava `model_override` + os demais
campos em `settings.toml`.

### 3.5.1 Estimativa de tempo antes do scan

Objetivo: projeção **com as escolhas atuais**, dentro do próprio modal, antes de iniciar.
Nenhuma tabela de tok/s embutida no código: a calibração roda **na máquina do usuário**, com
os mesmos caminhos de código do scan real.

```rust
pub struct VisionEstimate {
    pub files: FileCounts,        // images / videos / pdfs / others
    pub per_image_s: f64,
    pub per_video_s: f64,         // ~1 chamada VLM + ffprobe/ffmpeg
    pub total_s: f64,
    pub source: EstimateSource,   // Calibrated { samples: 3 } | Rough
    pub breakdown: Breakdown,     // gap_s + prefill_s + decode_s
}
```

Command `estimate_vision_scan(root_path, settings) -> VisionEstimate`:

1. **Contagem** — se o root já está indexado, usar `db` (instantâneo); senão walk
   metadata-only reaproveitando `collect_image_probes_incremental` em background.
2. **Calibração** — se não houver cache para
   `(model_digest, image_max_side, prompt_profile, gpu_signature)`, rodar **3 probes**
   pelo pipeline real: `prepare()` → `ollama_generate()` → ler `prompt_eval_duration` /
   `eval_duration` da resposta. Custo ~10–30 s, com progresso no modal ("medindo 2/3…").
   Fórmula por imagem:

   ```
   per_image_s = gap_s(medido) + prefill_s(medido) + decode_s(medido)
   total_s     = images * per_image_s + videos * per_video_s + pdfs * per_pdf_s
   ```

   Os probes são **estratificados**, não min/mediana/max: dividir as imagens do root em
   tercis por `size_bytes` (proxy barato de resolução, já disponível no walk) e pegar 1
   amostra de cada terço, ponderando a média pela população de cada tercil. Bibliotecas
   mistas (prints + fotos de celular + RAW) têm custo por imagem que varia ~5×, e média
   simples de 3 amostras erra feio.

   `images` = **arquivos que serão classificados**, não o total do root: reaproveitar
   `list_unclassified_files()` (o que o scan já faz) para que re-scan/refresh mostrem
   projeção próxima de zero. Tempo de thumbnailing fica fora da projeção e é rotulado
   como tal ("classificação: ~X h").

3. **Cache** — `~/.config/frank_sherlock/vision_calibration.json`, invalidado quando muda
   modelo/resolução/perfil ou a assinatura da GPU. Reabrir o modal fica instantâneo.
4. **Fallback `Rough`** — se o usuário não quiser esperar, estimar com constantes
   conservadoras rotuladas *"estimativa grosseira (±50 %)"*. A UI nunca exibe número
   calibrado e número grosseiro com a mesma aparência.

O modal atualiza a projeção **ao trocar qualquer opção**. Como a calibração é por
(modelo, resolução, perfil), trocar de opção refaz os probes daquela combinação — o cache
torna o vai-e-vem instantâneo depois da primeira vez; enquanto recalibra, manter o valor
anterior esmaecido com "recalculando…".

**Convivência com a ETA do scan.** [`computeEta()`](../sherlock/desktop/src/utils/scanEta.ts:42)
hoje usa a média desde o início da fase, sensível a picos (Surya, PDF de 200 páginas).
Passar a usar média móvel das últimas 20 amostras e **semear** com `per_image_s` calibrado
nos primeiros arquivos, para a ETA não saltar ao iniciar.

### 3.6 Modal de seleção de modelo

Backend (novas commands em [`lib.rs`](../sherlock/desktop/src-tauri/src/lib.rs)):
- `list_vision_models() -> Vec<VisionModelOption>` — `name`, `size_bytes` (de `/api/tags`),
  `installed: bool`, `recommended: bool`, `fits_vram: bool`, `load_hint`.
  `fits_vram = weights + 1024 MiB (overhead) + kv_estimate(num_ctx) <= vram_total_mib`
  (usar `GpuInfo` de [`platform/gpu.rs`](../sherlock/desktop/src-tauri/src/platform/gpu.rs);
  quando `unified_memory`, usar 75 % da RAM como em
  [`effective_memory_mib`](../sherlock/desktop/src-tauri/src/llm/model_selection.rs:48)).
- `set_vision_model(model: Option<String>)` — grava/limpa `model_override` em `settings.toml`
  (reaproveita [`save_user_config`](../sherlock/desktop/src-tauri/src/config.rs:118)).

Frontend: `ModelPickerModal.tsx` (padrão de [`ModelInfoModal`](../sherlock/desktop/src/components/modals/ModelInfoModal.tsx:24)),
aberto no clique de "Scan" quando há mais de um modelo instalado; mostra por modelo:
tamanho, badge "cabe na GPU / usa RAM", aviso quando `weights + KV > VRAM`
(*"7b em 12 GB usa ~3 GB de RAM — ~2× mais lento"*), e botão "Baixar 3b" reaproveitando o
fluxo de pull existente ([`run_model_download`](../sherlock/desktop/src-tauri/src/llm/management.rs:124)).

Nada de 3b/7b fixos na UI: a lista vem do Ollama, a recomendação vem de
[`recommended_model`](../sherlock/desktop/src-tauri/src/llm/model_selection.rs:133).

### 3.7 Duplicadas (fase 2, medir antes)

`dhash` já é calculado para todo arquivo em
[`compute_dhash`](../sherlock/desktop/src-tauri/src/thumbnail.rs:17). Antes da fase de
classificação, agrupar por Hamming ≤ 6 e classificar 1 representante por cluster, copiando
`media_type`/`description`/`confidence` para os demais (mantendo `extracted_text` vazio nos
copiados). Ganho depende da biblioteca — instrumentar quantos clusters existem e decidir
com número na mão.

### 3.8 Fora de escopo (PRs separados)

- **HEIC/NEF**: `SUPPORTED_EXTS` não tem `.heic` nem `.nef`, e o crate `image` não decodifica
  nenhum dos dois. Nas pastas testadas, **24 de 27 fotos ficam invisíveis**. Exige `libheif`
  (HEIC) e extração do preview JPEG embutido do NEF (mais leve que demosaicar). É feature,
  não otimização.
- **Vídeos**: não são descartados. `.mp4/.mkv/.mov/...` estão em `SUPPORTED_EXTS` e
  [`classify_video`](../sherlock/desktop/src-tauri/src/classify.rs:771) extrai 5 keyframes,
  classifica **1** com o VLM (~1 chamada) e soma legendas. Custo por vídeo ≈ 1 imagem grande
  + `ffprobe`/`ffmpeg` (o MP4 de 345 MB / HEVC 10-bit vai dominar pelo `ffmpeg`, não pelo LLM).

---

## 4. Ordem de implementação

1. `vision_prep.rs` + `num_ctx`/`num_predict`/`temperature` como parâmetros de
   `ollama_generate` → medir de novo com os mesmos scripts.
2. Prompt curto + `format: json` + redução das tentativas.
3. Gate do OCR + `ocr_engine`.
4. `VisionSettings` + `list_vision_models`/`set_vision_model` + modal.
5. Dedupe por dhash (com métrica antes/depois).
6. (Separado) HEIC/NEF.

## 5. Critérios de aceite

- [`_bench_vision/bench_vision.py`](../_bench_vision/bench_vision.py) e
  [`bench_big.py`](../_bench_vision/bench_big.py) devem mostrar, para o mesmo modelo,
  queda ≥ 50 % no `wall` e `spill = 0` para modelos que caibam na VRAM.
- Nenhuma constante de VRAM/GPU no código; tudo derivado de `GpuInfo` + `/api/tags`.
- Com GPU ausente/desconhecida o app continua funcionando (fallback CPU como hoje).
- Testes: `num_ctx_for` (tabela acima), gate do OCR (anime com texto **não** dispara),
  parse de `settings.toml` com as novas chaves, e o modal (1 modelo instalado → não abre).
