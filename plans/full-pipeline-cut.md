# Plano — o corte de §5.12, `document_photo` e Settings

Complementa [`pipeline-cpu-gpu-overlap.md`](./pipeline-cpu-gpu-overlap.md) §5.12 e §10.
Este documento é o desenho executável da Tarefa 1; as Tarefas 2–4 estão nas seções 6–9.

Estado de partida: HEAD `38f1817`, `scripts/pre-commit.sh` verde, uma alteração não
commitada em §5.12. **Passo 0 é commitar essa alteração** para que o corte comece de um
ponto limpo.

## 1. O que está no caminho hoje

`full` = descoberta → `insert_pending_rows` (só `New`, `ON CONFLICT DO NOTHING`) →
fase 2 mono-thread (fingerprint, `moved`, revive, thumbnail, EXIF, ffprobe, carimbo de
`scan_marker`) → fase 3 (delegada ao backlog encadeado) → cleanup por `scan_marker` →
job separado `backlog` (produtor/consumidor).

A fase 2 é o que sobra: ~4 h para 58 743 arquivos a 250 ms/arquivo, com a GPU ociosa.
E é a única coisa que hoje resolve três identidades: `moved`, revive por caminho e
`scan_marker` das linhas que não são `New`.

## 2. Decisão central: o pipeline roda **dentro** do job `full`

```
flowchart TD
    A[Discovery walk] --> B[insert_pending_rows para New]
    B --> C[revive_deleted_rows por caminho]
    C --> D[stamp Unchanged em lote]
    D --> E[reset_modified_rows em lote]
    E --> F[null dos thumb_path mortos]
    F --> G[mapa de candidatos a moved: linhas vivas com scan_marker diferente]
    G --> H[Pipeline: produtor completa a linha]
    H --> I[Consumidor GPU]
    I --> J{Job cancelado ou breaker}
    J -->|sim| K[return sem cleanup]
    J -->|nao| L[mark_missing_as_deleted]
    L --> M[complete job]
```

Por que dentro do mesmo job e não no encadeado:

1. **O cleanup precisa rodar depois do produtor.** `moved` só pode ser decidido quando
   existe o fingerprint, e o fingerprint passou a ser do produtor. Se o cleanup do `full`
   rodar antes, o caminho antigo é soft-deletado e a linha nova nasce pendente — exatamente
   o estado proibido em §5.12. Com o pipeline no mesmo job, o mapa de candidatos é montado
   antes do cleanup e o `moved` preserva id + descrição.
2. **Um `scan_marker` só.** Não há como carregar o marcador de um job para o outro sem
   plumbing novo; no mesmo job o marcador é o do `job`.
3. **A medição fica melhor**: `scripts/measure-run.py` resume *um* job, então
   descoberta + pipeline aparecem na mesma janela.

Consequência: `chain_backlog` deixa de significar "encadeia um segundo job" e passa a
significar **"o `full` continua no pipeline"** (`true`, padrão) ou **"o `full` para depois
da descoberta e deixa thumbnails/classificação para o botão Process Backlog"** (`false`).
O bloco de encadeamento em `lib.rs` (linhas 2246–2268) sai; a leitura da configuração
passa a ser feita em `scan.rs`, onde já existe para `delegate_classification`.

`refresh` **não muda**: continua descoberta → fase 2 → cleanup. É o modo que ainda
carimba, gera thumbnail e detecta `moved` sem classificar, e é a rede de segurança se o
corte regredir.

## 3. Ordem exata dentro do `full`

1. descoberta (inalterada, sem fingerprint);
2. `insert_pending_rows(New)` — já existe, dentro de uma transação;
3. `revive_deleted_rows` para `New` cujo `rel_path` casa com linha soft-deletada
   (mesmo critério de tamanho da fase 2: tamanho igual preserva a classificação,
   diferente limpa);
4. `stamp_unchanged` (o flush em lotes de 200 que já existe, agora sem excluir `heal`);
5. `reset_modified_rows` em lote: `mtime_ns`, `size_bytes`, `scan_marker`, limpa
   `description`/`extracted_text`/`canonical_mentions`/`confidence`/`lang_hint`/
   `classify_state`/`failure_*`/`dhash`, zera `fingerprint` e `thumb_path`;
6. `null_dead_thumb_pointers` para as linhas `unchanged` cujo `thumb_path` não existe
   mais (o antigo conjunto `heal`, agora resolvido no banco em vez de num caminho
   especial);
7. mapa de candidatos a `moved` = linhas vivas do root com `scan_marker <> marker`
   (elas são, por construção, as que não apareceram na descoberta);
8. pipeline sobre `list_backlog_rows_for_scan(root_id, marker)`;
9. cleanup com `mark_missing_as_deleted(marker)` — só se o job não foi cancelado e o
   breaker não abortou;
10. `complete_scan_job_by_id` com `added = new - moved`, `modified`, `unchanged`,
    `moved`, `deleted`.

Os passos 2–6 são *writes em lote*, sem I/O de arquivo além dos `stat` que a descoberta
de `heal` já faz hoje.

## 4. Mudanças por função

### `db.rs`

- `BacklogRow` ganha `fingerprint: String` e `scan_marker: i64`.
- `query_backlog_rows`: a lista de colunas e o mapeamento andam juntos —
  `image_width, image_height, fingerprint, scan_marker` no fim do `SELECT` e os quatro
  `row.get` correspondentes nas posições 9..12. Um teste de contagem de colunas evita a
  repetição do erro que corrompeu as duas tentativas anteriores.
- `set_file_identity(file_id, fingerprint, location_text, scan_marker)` — escreve
  identidade e carimbo numa só `UPDATE`.
- `list_backlog_rows_for_scan(root_id, scan_marker)` — reusa `query_backlog_rows` com
  `AND scan_marker = ?` no filtro (o filtro é nosso, nunca input do usuário).
- `stamp_modified_rows(root_id, marker, rows: &[(rel, mtime, size)])` — transação única.
- `revive_deleted_rows(root_id, marker, rows: &[FileRecordUpsert])` — mesma transação
  de `insert_pending_rows` ou imediatamente depois dela.
- `null_dead_thumb_pointers(file_ids: &[i64])` — transação única.
- `move_candidates(root_id, marker) -> Vec<MoveCandidate>` com
  `{ id, rel_path, fingerprint, has_description, thumb_path }`, só fingerprint não vazio.
- `merge_moved_row(new_id, old_id, abs_path, filename, mtime_ns, size_bytes, marker)`:
  `DELETE` da linha placeholder + `UPDATE` da antiga (`rel_path`, `abs_path`,
  `filename`, `mtime_ns`, `size_bytes`, `scan_marker`, `deleted_at = NULL`,
  `thumb_path = NULL`) + `refresh_fts`, tudo na mesma transação — separar as duas
  escritas viola `UNIQUE(root_id, rel_path)`.

### `scan.rs`

- `produce_backlog_items(tx, ctx, root_id, thumb_dir, block, cancel, counters,
  scan_marker, candidates)`. Recebe o marcador e o mapa.
- Identidade no produtor, só quando `row.fingerprint` está vazio:
  `fingerprint_file` + `exif::extract_location` (imagens estáticas) e
  `db::set_file_identity`. Falha de leitura → `unreadable`, nunca `failed`.
- `moved`: se o fingerprint novo casa com um candidato ainda não usado, o produtor
  chama `merge_moved_row` e continua o item com o **id antigo** e
  `needs_classification = !has_description`; a thumbnail é regenerada porque
  `thumb_path` ficou NULL (o arquivo antigo do cache é órfão, como já é hoje).
- Ramo legado (vídeo/PDF): passa a gerar thumbnail e, para vídeo, gravar
  `duration_secs`/`video_*`/`audio_codec` via `db::set_video_metadata` — hoje isso vem
  do `probe_to_minimal_record` da fase 2 e desapareceria junto com ela.
- `thumbnail_only` passa a ser um wrapper fino de
  `thumbnail_for_path(ctx, thumb_dir, caches, abs, rel_path, force, want_exif)` para o
  produtor usar o mesmo caminho de vídeo/PDF.
- O laço do `full` vira `prepare_scan_rows` (passos 2–7) + chamada ao pipeline; a fase 2
  fica atrás de `mode == ScanMode::Refresh` sem nenhuma alteração de comportamento.
- Contadores: `moved` sai do produtor por `AtomicU64` (como `thumbnailed`), e
  `added = new_count - moved`.

### Invariantes que não podem regredir

| invariante | como fica garantido |
|---|---|
| toda linha visitada tem `scan_marker` do job | passos 2–5 carimbam antes do pipeline; o produtor também carimba ao gravar identidade |
| `moved` não vira "nova pendente + antiga soft-deletada" | `merge_moved_row` antes do cleanup, em transação única |
| arquivo ilegível não é falha de classificação | `unreadable` continua fora de `is_permanent_failure` e do breaker |
| `abs_path` recalculado em toda troca de `rel_path` | `merge_moved_row` recebe `abs_path` do probe |
| cancelar não soft-deleta | cleanup só depois de um pipeline que terminou; cancelar retorna antes |
| `confidence` não é estado | seleção continua por `PENDING_CLASSIFICATION_SQL` |

## 5. Testes obrigatórios (antes de qualquer medição)

1. `cancelled_pipeline_leaves_rows_pending_and_alive` — 5 imagens, cancelamento pelo
   callback de tick (determinístico, sem Ollama): todas as linhas vivas,
   `summary.deleted == 0`, as não visitadas com `description = ''` e `thumb_path IS NULL`.
2. `full_mixed_folder_keeps_unchanged_and_moved` — pasta com arquivo inalterado,
   renomeado (linha antiga classificada, caminho antigo removido) e novo; roda
   descoberta + carimbos + produtor (consumidor falso, sem LLM) e depois o cleanup:
   o renomeado mantém id e descrição no caminho novo, o inalterado mantém tudo, o novo
   fica pendente com fingerprint gravado, `deleted == 0`.
3. Regressões que já existem: `healed_rows_survive_the_end_of_scan_cleanup`,
   `classify_only_keeps_rows_for_files_missing_on_disk`,
   `incremental_discovery_marks_*`, testes do produtor em `scan.rs`.
4. `db.rs`: ida-e-volta de `set_file_identity`, `merge_moved_row` (sem violar UNIQUE),
   `reset_modified_rows` limpando hash/fingerprint, `list_backlog_rows_for_scan`
   devolvendo o carimbo e o fingerprint certos.

## 6. Validação (sem queimar GPU)

Antes de qualquer coisa nos 58 mil:

1. `Refresh` em `Organizar/Fotos celular Stéphan/2020` (191 arquivos);
2. `Process Backlog` na mesma pasta;
3. `python3 scripts/measure-run.py 900` **imediatamente**, antes de outro job;
4. conferir `pending now`, `runner crashes: 0`, `model loads: 0–1`;
5. registrar o número em `_bench_vision/RESULTS.md` §7 e em §5.13 do plano principal.

## 7. Commits

Poucos e temáticos: (1) a alteração de §5.12, (2) o corte completo da Tarefa 1 com os
números da pasta de 191, (3) `document_photo` + migration 16, (4) Settings, (5) A/B 3b/7b.
Mensagens em inglês, corpo com o porquê e os números.

## 8. Tarefa 2 — `document_photo`

- `PRIMARY_PROMPT_CONCISE` e `PRIMARY_PROMPT_DETAILED` ganham
  `"document_photo":"none|receipt|invoice|statement|other"`; `is_document_like` sai do
  schema, do salvage e dos testes. O salvage em regex passa a extrair `document_photo`.
- `should_run_document_enrichment(primary, engine)`:
  - `media_type` em `document|screenshot` → enriquece (comportamento atual);
  - `document_photo` em `receipt|invoice|statement|other` → enriquece **mesmo em cena**
    (é assim que o comprovante fotografado volta);
  - `document_photo = none` → não enriquece (a quadra de vôlei para de pagar 13 s);
  - chave ausente (fallback de parse) → cai na regra antiga por `media_type`.
- `ClassificationResult.document_hint`, `update_file_classification(… document_hint)`,
  `copy_classification` copiando a coluna, migration 16
  (`ALTER TABLE files ADD COLUMN document_hint TEXT NOT NULL DEFAULT ''`).
  Default `''` = "não avaliado"; o bucket é
  `document_hint NOT IN ('', 'none')`, para um índice antigo não parecer cheio de
  documentos.
- Bucket de documentos: `list_document_rows(root_id)` e um segundo bloco de
  enriquecimento no pipeline (linhas de documento de um lado, para o A/B de §10 com
  `num_predict` menor e surya).

## 9. Tarefas 3 e 4

- Settings: comando `update_settings` recebendo a struct inteira (`SettingsUpdate` com os
  oito campos, validação pelos mesmos `parse`/`normalize`, retorno do
  `RuntimeSettings` salvo), `VisionSettings` do frontend com `imageFilter`,
  `dedupeDhashDistance` e `chainBacklog`, e um `SettingsModal` com os oito campos, CSS de
  `shared-modal.css` e teste próprio. O seletor de modelo com calibração continua no
  `ModelPickerModal`.
- A/B 3b vs 7b: 100 arquivos, `ScanOverrides` no `start_scan` (o comando já aceita
  overrides; falta expor no frontend) e um `scripts/dump-descriptions.py` que exporta
  `rel_path,media_type,description` por braço para comparação lado a lado; `measure-run.py`
  depois de cada braço.
