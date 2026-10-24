# Plano — não reescanear a pasta a cada abertura do app

> **Status: implementado.** `ScanMode::{Full,Refresh,ClassifyOnly}` em
> [`scan.rs`](../sherlock/desktop/src-tauri/src/scan.rs:66), `startup_scan` em
> [`config.rs`](../sherlock/desktop/src-tauri/src/config.rs:246), comandos
> `pending_scan_summary`/`set_startup_scan` e o
> [`StartupScanModal`](../sherlock/desktop/src/components/modals/StartupScanModal.tsx:1).
> 395 testes Rust e 359 frontend passam. Descoberta em background (seção 7) segue
> como follow-up.

## 1. Por que isso acontece hoje

O app só inicia scan automático quando recebe uma pasta por argumento
([`get_cli_folder_path()`](../sherlock/desktop/src-tauri/src/lib.rs:142) ← `std::env::args().nth(1)`).
Como a pasta vem na linha de comando, essa é a sequência a cada abertura:

1. [`recover_incomplete_scan_jobs()`](../sherlock/desktop/src-tauri/src/db.rs:315) converte todo job
   `running` em `interrupted` (o app morreu no meio da classificação de 53k arquivos).
2. [`initApp()`](../sherlock/desktop/src/App.tsx:189) encontra o job `interrupted` do root que veio
   por argumento e chama `startScan(cliPath)` **sem perguntar**.
3. [`create_or_resume_scan_job()`](../sherlock/desktop/src-tauri/src/db.rs:519) reaproveita o job, mas
   reseta `phase = 'discovering'` — e
   [`run_scan_job_internal()`](../sherlock/desktop/src-tauri/src/scan.rs:90) **sempre** roda a
   descoberta completa (`collect_image_probes_incremental`) antes de retomar a classificação.

Ou seja: o cursor de classificação é preservado, mas a caminhada de 53k arquivos no SMB é
refeita a cada abertura (~5 min), para nada.

## 2. O que muda

### 2.1 Novo modo de scan: `classify_only`

`start_scan` passa a receber um modo explícito em vez do booleano `skip_classify`:

| modo | descoberta | thumbnails | classificação | limpeza |
|---|---|---|---|---|
| `full` (hoje) | ✅ | ✅ | ✅ | ✅ |
| `refresh` (hoje `skip_classify`) | ✅ | ✅ | ❌ | ✅ |
| `classify_only` (novo) | ❌ | ❌ | ✅ | ❌ |

Regras de segurança do `classify_only` (as três são obrigatórias):

- **Nunca rodar a fase de limpeza.** `mark_missing_as_deleted(root_id, scan_marker)` marca como
  deletado tudo que não foi tocado nesta execução — sem descoberta, isso apagaria o índice inteiro.
- **Não depender do `scan_marker`.** `list_unclassified_files()` filtra `scan_marker = job.scan_marker`;
  se o job morreu durante a descoberta, nenhuma linha tem esse marker e a lista viria vazia.
  Nova função `list_unclassified_files_any_marker(db, root_id)` (`confidence = 0 AND deleted_at IS NULL`).
- **Fallback automático**: se `classify_only` encontrar 0 arquivos pendentes, o job termina como
  `completed` com 0 processados em vez de ficar "running" — o usuário vê que não havia nada a fazer.

O `phase` do job começa em `classifying` nesse modo, então a barra de progresso e a ETA continuam
funcionando sem mudanças.

### 2.2 Modal de abertura (o pedido)

Novo `StartupScanModal`, mostrado **apenas** quando há trabalho pendente e a pasta veio por
argumento (primeira indexação continua entrando direto no scan completo):

```
┌─ "Fotos" já está indexada ────────────────────────────────────────┐
│ 53.748 arquivos ainda não classificados                           │
│ Última verificação: há 2 dias                                      │
│                                                                    │
│ ● Continuar classificação   usa o índice atual, sem reescanear     │
│ ○ Verificar mudanças        descoberta completa (~5 min) + classif.│
│ ○ Só abrir                  navega pelo índice, nada é processado  │
│                                                                    │
│ ☐ Não perguntar novamente                                          │
│                                    [ Cancelar ]  [ Continuar ]     │
└────────────────────────────────────────────────────────────────────┘
```

- "Continuar classificação" → `beforeScan` (modal de modelo + projeção, fluxo já existente) →
  `startScan(mode = "classify_only")`.
- "Verificar mudanças" → mesmo fluxo com `mode = "full"`.
- "Só abrir" → nada é iniciado; o índice atual fica navegável e o "Rescan" da sidebar continua
  disponível.
- "Não perguntar novamente" grava `startup_scan` em `settings.toml`.

### 2.3 Preferência `startup_scan`

Em [`config.rs`](../sherlock/desktop/src-tauri/src/config.rs:154), junto das chaves de visão:

```toml
# O que fazer ao abrir a pasta por linha de comando:
#   ask           pergunta (padrão)
#   classify_only retoma a classificação pendente, sem reescanear
#   full          faz o scan completo
#   none          só abre
startup_scan = "ask"
```

Validação com fallback para `ask` (mesmo padrão tolerante das outras chaves). As opções também
ficam editáveis pelo modal de configuração existente.

### 2.4 Números no modal

- Contagem pendente: reusa [`vision_estimate_rows()`](../sherlock/desktop/src-tauri/src/db.rs:1131)
  (52 ms para 53k linhas) — nunca caminha no filesystem.
- "Última verificação": `roots.last_scan_at`.
- Projeção de tempo: já aparece no modal seguinte (calibrada). Não duplicar aqui.

## 3. Impacto de pular a descoberta (o que o usuário precisa saber)

| Sem descoberta | Consequência |
|---|---|
| Arquivos novos | não entram no índice até um scan completo |
| Arquivos removidos/renomeados | continuam listados; abrir/preview falha, classificação ignora e mantém `confidence = 0` |
| Thumbnails de novos arquivos | não são gerados até um scan completo |
| Detecção de mount caído / share offline | não roda (só existe dentro do scan) — mas `probe_roots()` no init continua marcando o root como Offline |
| Classificação, busca, álbuns, faces | **inalterados** — só linhas com `confidence = 0` são tocadas |

Nada é apagado: a fase de limpeza fica desligada nesse modo.

## 4. Arquivos afetados

**Backend**
- [`scan.rs`](../sherlock/desktop/src-tauri/src/scan.rs:81): `ScanMode` (enum serde), ramos de
  descoberta/thumbnail/limpeza condicionais, `list_unclassified_files_any_marker` no modo
  `classify_only`.
- [`db.rs`](../sherlock/desktop/src-tauri/src/db.rs:680): nova query sem filtro de marker;
  helper de contagem pendente por root.
- [`lib.rs`](../sherlock/desktop/src-tauri/src/lib.rs:310): `start_scan(root, mode)` e
  `spawn_scan_worker_if_needed(..., mode)`.
- [`config.rs`](../sherlock/desktop/src-tauri/src/config.rs:154): chave `startup_scan` + parse/save.
- Novo comando `pending_scan_summary(root) -> { unclassified, lastScanAt, hasIndex }` para o modal.

**Frontend**
- Novo `components/modals/StartupScanModal.tsx` (+ CSS, padrão dos outros modais).
- [`App.tsx`](../sherlock/desktop/src/App.tsx:189): `initApp` deixa de chamar `startScan` direto;
  resolve `startup_scan` da config e, quando `ask`, abre o modal.
- [`useScanManager.ts`](../sherlock/desktop/src/hooks/useScanManager.ts:47): `startScan(root, mode)`
  em todos os pontos (pick, rescan, refresh, resume).

## 5. Testes

Rust:
- `classify_only` não chama descoberta, não gera thumbnails e **não roda limpeza** (índice intacto
  depois de um job com arquivos faltando no disco).
- `classify_only` classifica linhas com marker antigo (a query sem marker).
- `classify_only` com 0 pendentes completa o job com 0 processados.
- Parse de `startup_scan` (válido, inválido → `ask`).

Frontend:
- Modal não aparece sem índice, nem quando não há pendências.
- "Só abrir" não chama `startScan`; "Continuar" chama com `classify_only`; "Verificar" com `full`.
- "Não perguntar novamente" persiste a preferência.

## 6. Ordem de implementação

1. `ScanMode` + ramos no `scan.rs` + query sem marker no `db.rs` + testes.
2. `start_scan(root, mode)` e `spawn_scan_worker_if_needed`.
3. `startup_scan` em `config.rs` + comando `pending_scan_summary`.
4. `StartupScanModal` + `initApp` + `useScanManager`.
5. Rodar `cargo test`, `clippy`, `tsc`, `npm test`.

## 7. Fora de escopo (follow-up)

- **Descoberta em background**: abrir o app instantaneamente e verificar mudanças depois, sem
  bloquear nem iniciar classificação automaticamente. É a solução definitiva para os 5 minutos,
  mas exige separar "verificar" de "classificar" no fluxo de jobs (hoje são fases do mesmo job).
- **Descoberta incremental mais barata**: evitar `stat` por arquivo em mounts de rede (ex.: usar
  `readdir` com d_type e só stat quando mtime do diretório mudou). Precisa de benchmark próprio
  antes de mexer.
