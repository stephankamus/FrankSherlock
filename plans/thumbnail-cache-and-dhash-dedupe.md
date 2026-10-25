# Plano — cache de thumbnails por raiz, auto-cura e dedupe por dhash

> **Status: implementado.** 407 testes Rust e 361 frontend passam; `clippy
> --all-targets` e `tsc --noEmit` sem avisos. Passo 5 de
> [`vision-classification-speed.md`](./vision-classification-speed.md:288)
> concluído (dedupe por dhash em thumbnails e na classificação).
>
> Correções entregues: cache em `cache/thumbnails/<root_key>/` com migração no
> boot, validação de cache sem mtime (origens com mtime no futuro), auto-cura de
> `thumb_path` morto no scan, dedupe por fingerprint/dhash com hardlink, move
> detection que não rouba a linha de cópias byte-idênticas, `[search_debug]` em
> `debug` e o comando `repair_thumbnails` com botão na sidebar.
>>>>> REPLACE

## 0. Sintoma medido (banco real do usuário)

| Medida | Valor |
|---|---|
| raízes no índice | **1** (`.../share=fotos/Fotos`), nenhuma linha duplicada |
| linhas em `files` | 53.771 (`abs_path` e `rel_path` todos distintos) |
| `thumb_path` apontando para arquivo inexistente | **32.030** |
| thumbnails presentes no cache | 21.594 (+2.194 órfãos = 23.788 arquivos) |
| `Eu e tay/` | 1.207 linhas, **0** arquivos de thumb |
| job de scan | `running` / `phase=thumbnailing`, cursor em `2015/31-10-2015 …`, sem escrita desde 19:09 |
| contadores do job | `processed_files=104.890` para `total_files=58.743`, `moved=6.032` |
| mtime dos arquivos no NAS | **2037-09-10** (relógio do servidor adiantado) |

## 1. Causas (todas confirmadas no código)

1. **Cache chaveado só por `rel_path`** — [`thumbnail.rs:43-44`](../sherlock/desktop/src-tauri/src/thumbnail.rs:43).
   Duas raízes que cobrem a mesma árvore resolvem para os mesmos arquivos, e
   `purge_root` ([`db.rs:1419-1424`](../sherlock/desktop/src-tauri/src/db.rs:1419)) /
   `cleanup_deleted_caches` ([`scan.rs:1236-1246`](../sherlock/desktop/src-tauri/src/scan.rs:1236))
   apagam por `thumb_path` — uma raiz destrói o cache da outra.
2. **Nenhuma verificação de existência para arquivos `Unchanged`** — o flush em
   [`scan.rs:322-331`](../sherlock/desktop/src-tauri/src/scan.rs:322) e o `continue`
   em [`scan.rs:355`](../sherlock/desktop/src-tauri/src/scan.rs:355) só tocam
   `scan_marker`; `generate_thumbnail` nunca é chamado de novo. Caminho morto é permanente.
3. **Validação de cache por mtime do arquivo** — `if t >= s` em
   [`thumbnail.rs:46-62`](../sherlock/desktop/src-tauri/src/thumbnail.rs:46) (idem
   `132-148` e `270-286`). Com mtime da origem em 2037 o thumb é sempre considerado
   vencido: o scan redecodifica tudo a cada execução e nunca converge.
4. **Move detection confunde cópias** — [`scan.rs:493-559`](../sherlock/desktop/src-tauri/src/scan.rs:493)
   renomeia a linha existente quando encontra o mesmo fingerprint em outro caminho,
   **mesmo que o caminho original ainda exista**. A original volta como `New` e
   perde a classificação (`moved=6.032`, inflando `added`).
5. **`[search_debug]` em INFO** — [`db.rs:2119`](../sherlock/desktop/src-tauri/src/db.rs:2119)
   dispara a cada poll da busca e polui o log.

## 2. Decisões

| Item | Decisão | Por quê |
|---|---|---|
| Layout do cache | `cache/thumbnails/<root_key>/<rel_path>.jpg` | elimina colisão entre raízes |
| `root_key` | 16 primeiros hex de `sha256(root_path canônico)` | estável ao remover/readicionar a raiz (diferente de `root_id`) |
| Invariante de `thumb_path` | só é gravado se o arquivo existe; ausente ⇒ `NULL` | deixa o reparo consultável por SQL e a UI honesta |
| Migração de layout | no boot, idempotente, com sentinela `.layout-v2` | 21,5 mil `rename` locais, custo desprezível |
| Validação do cache | sem comparação de mtime; `force` explícito da descoberta | mtime do SMB não é confiável (2037) |
| Dedupe de thumbnail | fingerprint igual ⇒ hardlink **sem ler do SMB**; dhash igual ⇒ hardlink depois do decode | cópias exatas custam zero; near-dupes economizam disco |
| Dedupe de classificação | Hamming ≤ `dedupe_dhash_distance` (padrão 6), 1 representante por cluster | passo 5 do plano de velocidade |
| Índice de similaridade | LSH com 7 bandas de 9 bits (garante achar toda distância ≤ 6) | exato e O(1) por arquivo |
| Escopo do dedupe | só `photo`/`screenshot` na classificação; fingerprint vale para qualquer tipo | dhash de vídeo/PDF é de 1 keyframe/página |
| Reparo | comando `repair_thumbnails` + botão na sidebar, com progresso e cancelamento | 32 mil arquivos levam horas no SMB |
| Log | `[search_debug]` → `log::debug!` | ruído em cada poll |

## 3. Desenho

### 3.1 `thumbnail.rs` — layout, chave de raiz e geração

```rust
/// 16 primeiros hex de sha256(root_path). Prefixo de diretório do cache.
pub fn root_key(root_path: &str) -> String

/// Caminho canônico do thumb de um arquivo (usado por scan, reparo e migração).
pub fn thumb_path_for(thumbnails_dir: &Path, root_key: &str, rel_path: &str) -> PathBuf

/// Gera (ou reutiliza) o thumb. `force = false` retorna o existente sem decodificar.
pub fn generate_thumbnail(
    source_path: &Path,
    thumb_dir: &Path,      // já é <thumbnails>/<root_key>
    rel_path: &str,
    force: bool,
) -> Option<ThumbnailResult>

/// Cria `thumb_dir/<rel>.jpg` como hardlink de `shared` (fallback: copy).
/// `dhash` vem do registro gêmeo, para não precisar decodificar a imagem.
pub fn reuse_thumbnail(
    shared: &Path,
    dhash: Option<u64>,
    thumb_dir: &Path,
    rel_path: &str,
) -> Option<ThumbnailResult>
```

- Remover o bloco `if thumb_path.exists() { … t >= s … }` (linhas 46-62, 132-148, 270-286).
  Sequência nova: `if !force && exists → Some(path, dhash: None)`; senão gera.
- Mesmo parâmetro `force` em `generate_pdf_thumbnail` e `generate_video_thumbnail`.
- `reuse_thumbnail` **não** decodifica nada: `create_dir_all` + `fs::hard_link`
  (mesmo dispositivo, portanto sempre funciona) com fallback `fs::copy`.
- Testes: `root_key` estável; `thumb_path_for` aninha por raiz; `force=false`
  não toca no arquivo existente; `reuse_thumbnail` compartilha o inode
  (`metadata.ino()` igual) e devolve `dhash` do gêmeo.

### 3.2 ScanContext e call sites

- `ScanContext::thumb_dir_for(&self, root_key: &str) -> PathBuf`
  ([`models.rs:345`](../sherlock/desktop/src-tauri/src/models.rs:345)).
- `run_scan_job_internal` calcula `let root_key = thumbnail::root_key(&job.root_path)`
  e `let thumb_dir = ctx.thumb_dir_for(&root_key)` uma vez
  ([`scan.rs:154`](../sherlock/desktop/src-tauri/src/scan.rs:154)).
- `thumbnail_only` recebe `thumb_dir: &Path` no lugar de `ctx.thumbnails_dir`
  ([`scan.rs:870`](../sherlock/desktop/src-tauri/src/scan.rs:870)); o mesmo no ramo de
  "moved" ([`scan.rs:512-533`](../sherlock/desktop/src-tauri/src/scan.rs:512)).
- `cleanup_deleted_caches` passa a receber `thumb_dir` e montar o caminho esperado com
  `thumb_path_for` ([`scan.rs:1242`](../sherlock/desktop/src-tauri/src/scan.rs:1242)).
- `lib.rs`: `retry_protected_pdfs` já tem `pdf.root_path`
  ([`models.rs:472`](../sherlock/desktop/src-tauri/src/models.rs:472)); `reclassify_pdf`
  passa a obter `root_path` via `db::get_root_path_for_file(file_id)` (novo).
  Call sites: [`lib.rs:862`](../sherlock/desktop/src-tauri/src/lib.rs:862) e
  [`lib.rs:938`](../sherlock/desktop/src-tauri/src/lib.rs:938).

### 3.3 Migração de layout no boot

Nova `db::migrate_thumbnail_layout(db_path, thumbnails_dir) -> MigrateThumbsResult`,
chamada em `init_database` (depois das migrações SQL) quando o app é gravável:

1. Se existe `thumbnails/.layout-v2`, não faz nada.
2. Para cada raiz: `key = root_key(root_path)`; para cada linha da raiz:
   - `new = thumb_path_for(thumbnails, key, rel_path)`;
   - se `new` já existe → `UPDATE … SET thumb_path = new`;
   - senão se `thumb_path` antigo existe → `create_dir_all` + `rename` + `UPDATE`;
   - senão → `UPDATE … SET thumb_path = NULL` (invariante; vira trabalho do reparo).
3. Remove diretórios vazios de primeiro nível que não são keys de raiz (best-effort).
4. Grava a sentinela. Retorna `{ moved, nulled }` e loga em INFO.

Ordem: migração roda **antes** de qualquer scan (o `spawn_scan_worker_if_needed`
só é chamado depois do `ensure_database`), então nenhuma linha é gravada com
layout antigo depois da sentinela.

### 3.4 Auto-cura no scan

- `ExistingFile` ganha `thumb_path: Option<String>` e `dhash: Option<i64>`
  ([`models.rs:333`](../sherlock/desktop/src-tauri/src/models.rs:333)); as duas queries
  (`load_existing_files` [db.rs:875](../sherlock/desktop/src-tauri/src/db.rs:875),
  `load_deleted_files` [db.rs:906](../sherlock/desktop/src-tauri/src/db.rs:906))
  passam a selecionar as colunas.
- Logo após a descoberta, calcular `heal: HashSet<String>` sobre **todos** os probes
  (inclusive os anteriores ao cursor): `existing.thumb_path` é `None` **ou**
  `!Path::new(tp).exists()`. `stat` só no cache local: ~53 mil chamadas, <1 s.
- O flush de `Unchanged` ([`scan.rs:322`](../sherlock/desktop/src-tauri/src/scan.rs:322))
  passa a excluir os `rel_path` de `heal`; eles entram no laço normal e são
  tratados como "só thumbnail": `force = true`, `update_file_thumb_and_dhash`,
  `scan_marker` tocado, sem reclassificação.
- Novo `db::update_file_thumb_and_dhash(db, root_id, rel_path, thumb, dhash)`.
- Contador `healed` no `ScanSummary` e no log ("healed 32.030 thumbs").
- Efeito colateral desejado: a primeira execução após o deploy repara as 32 mil
  linhas, mesmo com o cursor antigo do job, porque `heal` ignora o cursor.

### 3.5 Dedupe de thumbnails

Duas camadas, ambas alimentadas a partir do que o banco já sabe:

```rust
struct ThumbCaches {
    by_fingerprint: HashMap<String, (PathBuf, Option<u64>)>, // fingerprint → (thumb viva, dhash)
    by_dhash: HashMap<u64, PathBuf>,                         // dhash → thumb existente
}
```

- Semeadura: uma query por raiz retornando `fingerprint, dhash, thumb_path`
  das linhas com `thumb_path` não nulo; no Rust, descartar as que não existem.
- **Antes de gerar** (New/Modified, depois do fingerprint em
  [`scan.rs:388-429`](../sherlock/desktop/src-tauri/src/scan.rs:388)): se
  `by_fingerprint` tem o fingerprint → `reuse_thumbnail` e **nenhuma leitura do SMB**.
  É o caso dos 79 arquivos byte-idênticos e do álbum duplicado do usuário.
- **Depois de gerar** (dhash `Some`): se `by_dhash` já tem o dhash, apagar o arquivo
  recém-escrito e `reuse_thumbnail` a partir do existente (mesmo inode ⇒ 1 cópia no
  cache para N near-dupes). Caso contrário, inserir nas duas tabelas.
- Hardlinks por caminho próprio (nunca compartilhando `thumb_path` entre linhas),
  então `purge_root` e `cleanup_deleted_caches` continuam seguros: apagar um link
  não afeta o outro.

### 3.6 Move detection corrigido

Antes do laço, `let discovered: HashSet<&str> = probes.iter().map(|p| p.rel_path.as_str()).collect();`
No ramo de move ([`scan.rs:493`](../sherlock/desktop/src-tauri/src/scan.rs:493)),
exigir também `!discovered.contains(candidate.rel_path.as_str())`: só é movimento
quando o caminho antigo **desapareceu** da descoberta. Sem isso, uma cópia
byte-idêntica rouba a linha da original e a original volta com `confidence = 0`.

### 3.7 Dedupe na classificação (passo 5)

- `UnclassifiedFile` ganha `dhash: Option<i64>` e `media_type: String`
  ([`models.rs:201`](../sherlock/desktop/src-tauri/src/models.rs:201)); atualizar as
  duas queries ([`db.rs:686`](../sherlock/desktop/src-tauri/src/db.rs:686),
  [`db.rs:716`](../sherlock/desktop/src-tauri/src/db.rs:716)).
- `similarity::DhashIndex` (novo, no módulo que já trata near-dupes):

  ```rust
  pub struct DhashIndex { bands: [HashMap<u16, Vec<(u64, i64)>>; 7] }
  impl DhashIndex {
      pub fn insert(&mut self, dhash: u64, file_id: i64);
      /// Primeiro representante com distância de Hamming <= `threshold`.
      pub fn find_near(&self, dhash: u64, threshold: u32) -> Option<i64>;
  }
  ```
  7 bandas de 9 bits cobrem os 63 bits mais baixos; por pigeonhole, qualquer
  distância ≤ 6 compartilha ao menos uma banda. Confirmar a distância real com
  `(a ^ b).count_ones()` antes de aceitar.
- Semeia com as linhas já classificadas da raiz:
  `SELECT id, dhash FROM files WHERE root_id=? AND deleted_at IS NULL AND confidence > 0 AND dhash IS NOT NULL`.
- No laço ([`scan.rs:665`](../sherlock/desktop/src-tauri/src/scan.rs:665)), para
  `media_type` `photo`/`screenshot` com `dhash`:
  1. `find_near` → se achou representante: `db::copy_classification(target_id, source_id)`
     e `deduped += 1`, sem chamar o LLM;
  2. senão classifica normal e insere no índice.
- `db::copy_classification` (novo): copia `media_type, description, canonical_mentions,
  confidence, lang_hint` e grava `extracted_text = ''` (mesma regra do plano de velocidade).
- `ScanSummary` ganha `deduped: u64`; log final
  `"dedupe: {clusters} representantes, {deduped} arquivos dobrados"` — é a métrica
  antes/depois pedida pelo passo 5.
- `settings.toml`: `dedupe_dhash_distance = 6` (0 desliga), parse tolerante com
  fallback 6 em [`config.rs:154`](../sherlock/desktop/src-tauri/src/config.rs:154).

### 3.8 Comando `repair_thumbnails` + botão na sidebar

Backend, espelhando o padrão de face detection
([`lib.rs:1028`](../sherlock/desktop/src-tauri/src/lib.rs:1028),
[`models.rs:535`](../sherlock/desktop/src-tauri/src/models.rs:535)):

```rust
pub struct ThumbRepairProgress {
    pub root_id: Option<i64>, pub total: u64, pub processed: u64,
    pub repaired: u64, pub failed: u64,
    pub phase: String,   // "scanning" | "repairing" | "done" | "failed"
    pub error: Option<String>,
}
```

- `AppState` ganha `thumb_repair_progress: Arc<Mutex<Option<ThumbRepairProgress>>>`
  e `thumb_repair_cancel: Arc<AtomicBool>`
  ([`lib.rs:47`](../sherlock/desktop/src-tauri/src/lib.rs:47)).
- Commands: `repair_thumbnails(root_id: Option<i64>)`, `thumb_repair_status()`,
  `cancel_thumb_repair()`. Recusa quando há scan rodando
  (`running_scan_jobs` não vazio), como `compact_database`
  ([`lib.rs:464`](../sherlock/desktop/src-tauri/src/lib.rs:464)) e quando a raiz
  está offline (`probe_dir`, [`scan.rs:141`](../sherlock/desktop/src-tauri/src/scan.rs:141)).
- Passos: carregar `(id, root_id, rel_path, abs_path, media_type, fingerprint, dhash,
  thumb_path)` da(s) raiz(es); filtrar em Rust os que precisam
  (`thumb_path IS NULL` ou arquivo ausente) → `total`; para cada um, `force = true`
  com as mesmas `ThumbCaches` do scan (as cópias exatas do álbum duplicado custam
  só um hardlink); `update_file_thumb_path`/`update_file_thumb_and_dhash`;
  falha individual não aborta.
- `db::list_thumb_candidates(root_id)` (nova query) e `db::clear_thumb_path(file_id)`.

Frontend:
- `api.ts`: `repairThumbnails(rootId?)`, `getThumbRepairStatus()`, `cancelThumbRepair()`.
- `hooks/useThumbRepair.ts` no molde de
  [`useFaceDetection.ts:20-37`](../sherlock/desktop/src/hooks/useFaceDetection.ts:20)
  (poll + toast de falha uma única vez).
- `Sidebar.tsx`: botão `Repair Thumbnails` na lista de ferramentas
  ([`Sidebar.tsx:180-208`](../sherlock/desktop/src/components/Sidebar/Sidebar.tsx:180)),
  desabilitado em `readOnly` ou sem raízes; rótulo vira
  `Repairing 1.234/32.030` com um botão de cancelar, e
  `Repaired 32.030 thumbnails` ao terminar.
- `App.tsx` repassa props como já faz com `onCompactDatabase`
  ([`App.tsx:430`](../sherlock/desktop/src/App.tsx:430), [`App.tsx:869`](../sherlock/desktop/src/App.tsx:869)).

### 3.9 Job travado / contadores

- `recover_incomplete_scan_jobs` já converte `running` → `interrupted` no boot
  ([`db.rs:315`](../sherlock/desktop/src-tauri/src/db.rs:315)); com a auto-cura o
  resume passa a consertar as linhas pré-cursor, então não é preciso apagar o job.
- Corrigir a contabilidade: em [`scan.rs:281-333`](../sherlock/desktop/src-tauri/src/scan.rs:281)
  `processed_files` acumula o flush de `Unchanged` a cada resume, chegando a
  `processed > total`. Passar a reportar `processed_files = start_index + (processados
  nesta execução)` e não somar `unchanged_total` ao contador do job.

## 3.10 Desempenho do pipeline (medido em 2026-10-25)

Sintoma relatado: ~400 thumbnails/hora (~3,5 s por foto) e a impressão de que levaria dias.

| Medida | Valor |
|---|---|
| CPU do app durante a geração | **100 % de um core**, `iowait` 0 %, 8 % de uso total da máquina |
| CPU acumulada no processo | 157 min |
| leitura crua do share (25 fotos de 1–4 MB) | 13,5 MB/s, **0,19 s/arquivo** |
| leitura paralela ×4 | 15 MB/s (o share aceita concorrência) |
| `ffmpeg -ss … -frames:v 1` em vídeos de 35–134 MB | **0,2–0,4 s** |
| thumbnail completo com o novo perfil | **0,552 s/foto** (25 fotos reais) |

O gargalo era **CPU de decodificação**, não rede — e a causa principal era o build de
desenvolvimento (`tauri dev`), que compila tudo sem otimização. Correções:

1. `[profile.dev] opt-level = 1` + `[profile.dev.package."*"] opt-level = 3`
   ([`Cargo.toml`](../sherlock/desktop/src-tauri/Cargo.toml:17)).
2. Downscale com `imageops::thumbnail` (filtro box) em vez de `resize(…, Lanczos3)`
   ([`thumbnail.rs:36`](../sherlock/desktop/src-tauri/src/thumbnail.rs:36)) — invisível a 300 px.
3. Thumbnail de vídeo pede **1** keyframe em vez de 3
   ([`thumbnail.rs:352`](../sherlock/desktop/src-tauri/src/thumbnail.rs:352)).
4. `repair_missing_thumbnails` roda **4 workers** em blocos contíguos, cada um com suas
   caches de dedupe ([`scan.rs:1613`](../sherlock/desktop/src-tauri/src/scan.rs:1613)).
5. Fotos e PDFs primeiro, vídeos por último
   ([`scan.rs:1596`](../sherlock/desktop/src-tauri/src/scan.rs:1596)).
6. Cura de arquivos **antes do cursor** do job, que o laço de retomada nunca visitava
   ([`scan.rs:392`](../sherlock/desktop/src-tauri/src/scan.rs:392)).
7. Barra do reparo mostra vazão e ETA (`n/min, ~Xh`).

Resultado: ~6× mais rápido por foto e ~4× em paralelo; as 30.720 linhas pendentes do
usuário saem de "dias" para 1–2 horas.

## 4. Schema e configuração

- **Sem coluna nova.** `dhash` já existe com índice
  ([`db.rs:222-223`](../sherlock/desktop/src-tauri/src/db.rs:222)).
- `settings.toml`: `dedupe_dhash_distance` (0–16, padrão 6).
- Migração de cache é de **filesystem**, versionada por sentinela — não mexe em
  `user_version` (continua 14).

## 5. Testes

Rust:
- `root_key` determinístico e `thumb_path_for` aninhado; raízes diferentes nunca
  colidem no mesmo caminho.
- `generate_thumbnail(force=false)` com mtime da origem no futuro **não** regenera;
  `force=true` regenera.
- `reuse_thumbnail` cria hardlink (mesmo inode) e devolve o dhash do gêmeo.
- Migração: linha com thumb existente é movida e `thumb_path` atualizado; linha com
  thumb ausente vira `NULL`; segunda execução é no-op (sentinela).
- Scan: arquivo `Unchanged` com `thumb_path` morto é regenerado e conta em `healed`;
  arquivo `Unchanged` com thumb vivo não é tocado.
- Dedupe: duas linhas com o mesmo fingerprint geram **um** arquivo no cache (mesmo
  inode) e nenhuma leitura da segunda origem (assert com `Path` inexistente de
  propósito).
- Move: cópia idêntica não renomeia a linha da original (regressão de `moved`).
- `DhashIndex`: acha todos os pares com distância ≤ 6 em um conjunto sintético,
  não acha distância 7; `copy_classification` copia os campos e zera `extracted_text`.
- `repair_thumbnails`: com 3 linhas mortas e 1 viva, repara 3, `failed = 0`,
  status vai a `done`; cancelamento aborta com fase `failed`/`cancelled`.
- Parse de `dedupe_dhash_distance` válido/inválido.

Frontend:
- Botão desabilitado em `readOnly` e sem raízes.
- Clique chama `repairThumbnails`; progresso aparece no rótulo e o cancelamento
  chama `cancelThumbRepair`.

## 6. Ordem de implementação

1. `thumbnail.rs`: `root_key`, `thumb_path_for`, `reuse_thumbnail`, `force` (com testes).
2. Layout por raiz + `ScanContext::thumb_dir_for` + call sites de `scan.rs` e `lib.rs`.
3. `db::migrate_thumbnail_layout` no boot (+ teste de idempotência).
4. Auto-cura no scan (`ExistingFile.thumb_path/dhash`, `heal`, `update_file_thumb_and_dhash`).
5. Dedupe de thumbnails (fingerprint e dhash) + `move` corrigido + `[search_debug]` → debug.
6. Dedupe de classificação (`DhashIndex`, `copy_classification`, `dedupe_dhash_distance`, `deduped`).
7. `repair_thumbnails` (backend) e botão na sidebar (frontend).
8. `cargo test`, `cargo clippy`, `tsc`, `npm test`; atualizar o status do passo 5 em
   [`vision-classification-speed.md`](./vision-classification-speed.md:317).

## 7. Fora de escopo

- Dedupe por dhash para vídeos e PDFs (só o atalho por fingerprint vale para eles).
- Indicador "duplicata de X" na UI (badge no tile / campo em Properties).
- Dedupe entre raízes diferentes: o cluster é sempre por raiz, como a classificação.
- HEIC/NEF (passo 6 do plano de velocidade).
- Descoberta em background ([startup-scan-prompt.md §7](./startup-scan-prompt.md:157)).
