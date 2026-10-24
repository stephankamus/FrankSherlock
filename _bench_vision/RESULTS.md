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
