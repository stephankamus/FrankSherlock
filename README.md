<p align="center">
  <img src="docs/app_icon.png" alt="Frank Sherlock" width="128" />
</p>

# Frank Sherlock — fork pessoal / personal fork

## Sobre este fork (PT-BR)

Este repositório é um fork de uso pessoal do [FrankSherlock](https://github.com/akitaonrails/FrankSherlock). O projeto original é excelente para catalogar uma biblioteca local, mas o meu cenário é outro: mais de 50 mil fotos em um NAS via SMB, acessadas de uma máquina modesta, sem GPU de ponta, e com um índice grande o suficiente para que qualquer rescaneamento completo doesse. Por isso, adaptei o aplicativo para que o caminho do dia a dia — abrir a biblioteca, filtrar, buscar e visualizar — fosse o mais rápido possível, deixando a indexação pesada rodar em segundo plano e de forma retomável.

As principais mudanças em relação ao projeto original são: pipeline de scan com um único decode por foto gerando miniatura e imagem de visão, sobreposição de CPU e GPU no backlog, cache de miniaturas por raiz com auto-cura e deduplicação por dHash, otimizações no request de visão (downscale, `num_ctx` derivado da imagem e prompt curto), conversão opcional de HEIC/NEF para JPEG, endurecimento de shares de rede (raízes offline, relink e compactação do banco), descrições em português, fotos ocultas, visualizador de logs ao vivo, prompt de retomada ao abrir uma pasta já indexada e — a maior adição — uma publicação de snapshot imutável com um cliente web somente leitura (PWA) com login por token, para abrir a biblioteca no celular ou em outra máquina sem GPU.

Não pretendo publicar releases nem acompanhar o updater do repositório de origem: as alterações locais seriam sobrescritas. Por isso o botão **Check for Updates** fica oculto na interface. O plugin do updater continua no código para o dia em que este fork tiver o próprio canal de atualização. A versão atual deste fork é **0.9.0**.

## About this fork (EN-US)

This repository is a personal-use fork of [FrankSherlock](https://github.com/akitaonrails/FrankSherlock). The original project is excellent for cataloging a local library, but my scenario is different: 50k+ photos on an SMB/NAS share, browsed from modest hardware with no high-end GPU, and an index large enough that any full rescan was expensive. I adapted the application so the everyday path — open the library, filter, search and preview — is as fast as possible, while the heavy indexing runs in the background and can resume after interruption.

The main changes over the original project are: a scan pipeline that decodes each photo once to produce both the thumbnail and the vision image, CPU/GPU overlap in the backlog pass, a per-root thumbnail cache with self-healing and dHash deduplication, faster vision requests (downscale, image-derived `num_ctx` and a short prompt), opt-in HEIC/NEF to JPEG conversion, network-share hardening (offline roots, relink and database compaction), Portuguese descriptions, hidden photos, a live log viewer, a startup prompt when opening an already indexed folder, and — the largest addition — immutable snapshot publishing with a read-only web client (PWA) and token login, so the library opens on a phone or on another machine without a GPU.

I do not intend to publish releases or follow the upstream updater: local changes would be overwritten. For that reason the **Check for Updates** button is hidden in the UI. The updater plugin stays in the code for the day this fork gets its own update channel. The current version of this fork is **0.9.0**.

---

# Frank Sherlock (Português)

Catalogador e buscador de imagens com IA, local-only, para usar em um NAS. Aponte o aplicativo para um diretório e ele classifica cada imagem com um modelo de visão local, extrai texto por OCR, gera miniaturas e indexa tudo em um banco SQLite pesquisável. Nada sai da sua máquina.

## O que ele faz

- Escaneia diretórios de imagens, PDFs e vídeos em modo somente leitura (JPEG, PNG, GIF, BMP, WebP, TIFF, PDF, MP4, MKV, AVI, MOV, WebM e outros)
- Converte HEIC e NEF em cópias JPEG sob demanda (`FrankSherlock-JPG/` ao lado dos originais), sem alterar os arquivos originais, para que fotos de celular e raws de câmera entrem no índice
- Classifica cada imagem com um modelo de visão local do Ollama (tipo de mídia, descrição, identificação de anime/mangá, detecção de documentos)
- Extrai texto com Surya OCR (com fallback para o modelo de visão) em documentos, recibos e capturas de tela
- Gera miniaturas JPEG de 300 px para navegação rápida (montagem de 2 páginas para PDFs)
- Busca full-text com SQLite FTS5
- Encontra duplicados exatos por fingerprint de conteúdo e quase-duplicados por hash perceptual (dHash) + similaridade de descrição
- Detecta e reconhece rostos com modelos ONNX nativos (SCRFD + ArcFace), agrupa rostos em pessoas, permite nomear e mesclar, e buscar por `face:nome`
- Detecta arquivos renomeados/movidos por fingerprint, sem reclassificá-los
- Retoma scans interrompidos a partir do último checkpoint
- Mostra o progresso da descoberta ao vivo em diretórios grandes
- Navega por subdiretórios com uma árvore recolhível no sidebar
- "Refresh Metadata" refaz a varredura sem Ollama (somente miniaturas, sem LLM)
- Converte HEIC/NEF de forma opcional e reversível: as cópias são JPEGs comuns, marcadas com a origem na grade e ignoradas como "keepers" automáticos na tela de duplicados
- Oculta fotos da biblioteca (somente no desktop) com um filtro "mostrar ocultas"
- Publica snapshots imutáveis do índice para um cliente web somente leitura, com revisões e ponteiro atômico
- Abre a biblioteca no navegador ou no celular como PWA, com login por token e preview de imagem, vídeo (Range) e PDF
- Escolhe o modelo, o filtro de resize, o idioma das descrições, o motor de OCR e o comportamento de retomada em uma tela de Settings
- Mostra um botão de reparo de miniaturas e um botão de compactação do banco para bibliotecas grandes em share de rede
- Exibe logs ao vivo durante um scan e ETA estimado para a fila de classificação

## Captura de tela

![Frank Sherlock screenshot](docs/frank_sherlock.png)

A interface é levemente inspirada no VS Code: barra de título própria, sidebar recolhível, grade de miniaturas, filtros por tipo de mídia, overlay de preview, localizador de duplicados e tema claro/escuro automático.

## Instalação

Este fork **não publica binários**. Os releases do repositório de origem não incluem as mudanças descritas acima. A forma recomendada é compilar a partir do código (veja "Compilando a partir do código").

Você também precisa do [Ollama](https://ollama.com) instalado e em execução (`ollama serve`). Na primeira inicialização, o aplicativo oferece baixar o modelo de visão caso ele ainda não exista.

### Aviso do Windows SmartScreen

O instalador MSI do upstream não é assinado com certificado de code signing. O SmartScreen pode bloqueá-lo com "O Windows protegeu o seu PC". Clique em "Mais informações" e depois em "Executar assim mesmo". O mesmo vale para qualquer build local deste fork.

## Requisitos (para compilar a partir do código)

- Linux, macOS ou Windows
- [Ollama](https://ollama.com) instalado e em execução (`ollama serve`)
- GPU NVIDIA recomendada no Linux/Windows (séries RTX funcionam bem com `qwen2.5vl:7b`); Apple Silicon funciona nativamente no macOS
- GPU é opcional: em hardware modesto, `qwen2.5vl:3b` é a escolha recomendada e o Ollama também pode rodar em CPU (mais lento)
- Node.js 20+
- Rust 1.77+

## Configuração do Ollama

O Frank Sherlock cria um arquivo de configuração comentado na primeira inicialização:

- Linux: `~/.config/frank_sherlock/settings.toml`
- macOS: `~/Library/Application Support/frank_sherlock/settings.toml`
- Windows: `%APPDATA%\frank_sherlock\settings.toml`

Por padrão, o aplicativo escolhe um modelo de visão a partir do hardware detectado. Para forçar um modelo menor ou alternativo, edite o arquivo (as mudanças valem no próximo scan):

```toml
model_override = "qwen2.5vl:3b"
```

O mesmo arquivo controla como as imagens são enviadas ao modelo:

```toml
# Maior lado em pixels; 0 envia o arquivo original sem alteração.
# Valores menores são muito mais rápidos porque o pré-processamento de imagem
# do Ollama (e não o modelo) domina o custo de fotos grandes. 1024 mantém
# assuntos pequenos e texto legíveis.
image_max_side = 1024

# "concise" (padrão) ou "detailed" (adiciona candidatos de série/personagem de anime).
prompt_profile = "concise"

# Idioma da descrição (o que a busca encontra): "pt_br" (padrão) ou "en".
# Somente texto livre segue essa opção: tipos de mídia, valores de enum, texto
# de OCR e marcadores no estilo "[Doc: ...]" continuam em inglês. Linhas já
# descritas mantêm o idioma — "Convert existing descriptions" na tela de
# settings as reescreve com uma passada só de texto (minutos, sem ler imagem).
description_language = "pt_br"

# OCR para capturas/documentos: "off", "llm" (padrão, rápido) ou "surya"
# (melhor recall, mas abre um processo Python por imagem).
ocr_engine = "llm"

# O que fazer ao abrir uma pasta que já está indexada:
# "ask" (padrão), "classify_only" (retoma o trabalho pendente sem varrer a
# árvore de novo), "full" ou "none".
startup_scan = "ask"
```

Quando uma pasta é aberta por argumento de linha de comando e já está indexada, o aplicativo nunca rescaneia sozinho: ele pergunta se deve retomar a classificação pendente, verificar mudanças (o que varre a árvore de novo — lento em shares de rede) ou apenas abrir o índice existente. O caminho "Continuar classificação" pula descoberta, miniaturas e limpeza, então nada no disco é tocado e nada pode ser apagado.

Tudo isso também é editável pelo aplicativo: abra a barra de status de modelo e hardware e clique em "Classification settings". O mesmo diálogo mostra a lista de modelos que cabem na GPU atual e, antes de um scan começar, uma projeção de tempo para aquela pasta medida na sua própria máquina (algumas imagens reais são classificadas para calibrar; o resultado é cacheado).

Quando `model_override` está definido, o setup exige exatamente aquele modelo do Ollama. Se ele estiver faltando, o diálogo de primeira execução oferece baixá-lo com o Ollama antes de os scans começarem. A primeira requisição de classificação carrega o modelo sob demanda, e o aplicativo descarrega modelos do Ollama ao final dos scans.

Valores recomendados para o override:

- `qwen2.5vl:3b`: mais rápido e seguro para pouca VRAM, Windows NVIDIA <= 8 GB ou fallback em CPU.
- `qwen2.5vl:7b`: melhor qualidade quando o Ollama consegue carregá-lo inteiro na GPU.
- `qwen2.5vl:32b`: pesado; realista apenas em sistemas com muita memória unificada.

Alternativas experimentais como `minicpm-v:8b` ou `moondream` podem funcionar se estiverem instaladas no Ollama, mas os prompts são ajustados para `qwen2.5vl`.

## Compartilhamentos de rede e caminhos de NAS

As raízes são indexadas por caminho, então shares exigem cuidado. Um share que está desmontado na inicialização, depois de suspender ou depois de um logout não é mais expurgado: ele é mantido e marcado como `Offline` no sidebar, e `Locate…` reposiciona a raiz existente em um novo ponto de montagem mantendo classificação, álbuns, duplicados e rostos. Scans se recusam a rodar quando a descoberta cai para zero arquivos, em vez de apagar o índice.

Veja [docs/network-shares-linux.md](docs/network-shares-linux.md) para configuração de automount com systemd, ressalvas de GVFS, relink e compactação do banco.

## Cliente web (somente leitura)

O cliente web é uma visão somente leitura de uma biblioteca publicada pelo desktop. O desktop continua sendo o único que escreve no índice; o container apenas lê um snapshot imutável.

1. **Publique pelo desktop:** Settings → **Publicação**. Escolha uma pasta no share de rede (`publish_dir`) que não esteja dentro de uma raiz de fotos. Mantenha "Publish after every scan" ligado e escolha quantas revisões manter (padrão 2).
2. **Configure o mapeamento de mídia:** copie `config/media.map.example.json` para `config/media.map.json` e mapeie cada `storedPath` (como o desktop o registrou) para um `mountedPath` dentro do container.
3. **Suba o container:**

```bash
cd sherlock/web
cp .env.example .env       # defina AUTH_TOKEN e os caminhos do host
docker compose up -d --build
```

4. **Entre pelo navegador:** abra a URL do serviço e informe o mesmo `AUTH_TOKEN` na tela de login. O token é guardado no `localStorage` do dispositivo. Um link com `?token=...` continua funcionando para o primeiro acesso e é removido da barra de endereços depois de armazenado.

O cliente web inclui busca com filtros e ordenação, grade com scroll infinito, sidebar de raízes, subdiretórios, álbuns e smart folders, visualização de rostos, duplicados informativos, preview de imagem/vídeo/PDF, layout responsivo com drawer, manifest PWA e service worker do app shell. A API é sempre network-only; não existe biblioteca offline.

O token do aplicativo é uma segunda barreira, não a principal: coloque um reverse proxy na frente (Caddy, Traefik, nginx) com Basic Auth ou Authentik e TLS. Sem isso, qualquer dispositivo da LAN lê a biblioteca inteira. `AUTH_TOKEN` vazio desabilita a checagem e registra um `WARN` no log; use apenas para depuração.

Veja [docs/web-deploy.md](docs/web-deploy.md) para o passo a passo completo, incluindo SMB/CIFS, GVFS, diagnóstico de 401/503 e instalação no celular.

## Compilando a partir do código

Para um passo a passo completo em Ubuntu (Rust, bibliotecas de sistema, Node e Ollama), veja [docs/dev-setup-linux.md](docs/dev-setup-linux.md). Para produzir um `.deb` ou AppImage instalável, veja [docs/build-and-install-linux.md](docs/build-and-install-linux.md).

```bash
# 1. Instale as dependências do workspace (a partir da raiz do repositório)
npm install

# 2. Baixe as bibliotecas nativas da sua plataforma
cd sherlock/desktop/src-tauri
bash scripts/download-pdfium.sh
bash scripts/download-onnxruntime.sh
cd ../../..

# 3. Inicie o Ollama (em outro terminal)
ollama serve

# 4. Rode em modo de desenvolvimento
cd sherlock/desktop
npm run tauri:dev
```

Para gerar um binário de release (AppImage/DMG/MSI):

```bash
cd sherlock/desktop
npm run tauri:build
```

A saída fica em `sherlock/desktop/src-tauri/target/release/bundle/`.

### Contorno para Wayland/NVIDIA

Se a janela do WebKit ficar em branco no Wayland com drivers NVIDIA:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 GDK_BACKEND=wayland,x11 npm run tauri:dev
```

## Testes

```bash
# Frontend compartilhado
npm run test:shared

# Cliente web
npm run test:web

# Frontend do desktop
npm run test:desktop

# Backend Rust do desktop
cd sherlock/desktop && npm run test:rust

# Núcleo de leitura compartilhado
cd sherlock/core && cargo test
```

A suíte cobre parsing do JSON de classificação, geração de miniaturas, scans incrementais, operações do banco (upsert, touch, delete, FTS), cancelamento de scan, parsing de consultas, detecção de duplicados, similaridade, metadados de vídeo, detecção/agrupamento de rostos, publicação de snapshots, o contrato HTTP do cliente web e os componentes de UI dos dois frontends.

## Como funciona

### Scan incremental

Feito para diretórios grandes de NAS com 100 mil arquivos ou mais:

1. **Descoberta** — percorre a árvore usando apenas metadados do filesystem (mtime, tamanho), sem ler arquivos. Metadados do WalkDir são reaproveitados para evitar `stat` redundantes. Subárvores de outras raízes são puladas cedo via `filter_entry()`. O progresso é enviado à UI ao vivo a cada 500 arquivos.
2. **Miniaturas e pipeline** — arquivos novos e modificados são fingerprintados, miniaturizados e inseridos no banco com registros mínimos (confidence=0). Arquivos movidos são detectados por fingerprint e apenas atualizam a referência de caminho. Marcadores de arquivos inalterados são gravados em lote antes de começar o trabalho pesado.
3. **Classificação** — arquivos não classificados vão para o Ollama e seus registros são atualizados com metadados ricos. Essa fase pode ser pulada por completo com "Refresh Metadata", para rescans rápidos e sem Ollama.
4. **Limpeza** — arquivos que não estão mais no disco são soft-deleted e suas miniaturas em cache são removidas.

Cada fase suporta cancelamento e retomada independentes, com checkpoint a cada arquivo. Re-escanear um diretório inalterado de 10 mil imagens leva segundos.

### Otimizações deste fork para bibliotecas grandes

- **Um decode, dois artefatos:** a miniatura de 300 px e o JPEG de 1024 px usado pela visão saem do mesmo decode e da mesma leitura do arquivo. Antes, cada foto era lida e decodificada duas vezes (especialmente caro em SMB).
- **Pipeline de CPU/GPU:** no modo backlog, um produtor de CPU prepara o próximo arquivo enquanto o Ollama ocupa a GPU, reduzindo a ociosidade dos dois lados.
- **Request de visão mais barato:** downscale antes do envio, `num_ctx` derivado do tamanho da imagem, prompt curto com `format: json`, `num_predict` menor e um gate de OCR que evita disparar Surya em qualquer imagem com texto.
- **Deduplicação por dHash:** fotos visualmente iguais (dentro de uma distância de Hamming configurável) herdam a classificação do representante; miniaturas idênticas não são decodificadas de novo e são compartilhadas por hardlink.
- **Cache de miniaturas por raiz:** `cache/thumbnails/<root_key>/<rel_path>.jpg`, com migração idempotente no boot, validação que não depende de mtime do SMB e auto-cura de `thumb_path` morto.
- **Reparo e compactação:** o comando "Repair Thumbnails" regenera apenas o que falta, com progresso e cancelamento; "Compact DB" recupera espaço com VACUUM.
- **Filtro de resize configurável:** box para JPEG/HEIC (mais rápido) ou Lanczos3 para PNG/BMP/TIFF, com a qualidade como critério.

### Detecção de duplicados

Encontre e remova cópias redundantes para recuperar espaço:

- **Duplicados exatos** — agrupa arquivos com fingerprints SHA-256 idênticos. Uma heurística escolhe como keeper o arquivo mais antigo com o caminho mais curto.
- **Quase-duplicados** — similaridade perceptual usando dHash (calculado na geração da miniatura), combinada com sobreposição de palavras (Jaccard) nas descrições do LLM (85% visual + 15% textual). Union-Find para agrupamento transitivo.
- **Comparação de grupo** — preview lado a lado de todos os arquivos de um grupo, com metadados por arquivo.
- **Coloração em 3 níveis** — verde (exato, seguro apagar), amarelo (near >= 85%), vermelho (mais baixo, requer conferência visual).

### Detecção e reconhecimento de rostos

Encontre e organize pessoas na sua biblioteca:

1. **Detecção** — SCRFD (ONNX) encontra rostos com bounding boxes e 5 keypoints. Os modelos são baixados automaticamente no primeiro uso a partir do pacote buffalo_l do InsightFace.
2. **Reconhecimento** — ArcFace produz embeddings de 512 dimensões para cada rosto detectado, permitindo comparação por identidade.
3. **Agrupamento** — similaridade de cosseno (threshold 0.45) agrupa rostos em pessoas. Rostos novos entram em clusters existentes ou iniciam novos.
4. **Gestão** — nomeie pessoas, mescle duplicados e remova falsas detecções. A FacesView mostra todas as pessoas com crops representativos.
5. **Busca** — `face:alice`, `face:"Nome Completo"` ou `face:42` (por ID da pessoa) para achar todas as fotos de alguém.

A detecção de rostos roda por pasta pelo menu de contexto, mostra progresso na barra de status e pode ser executada dentro do próprio scan.

### Pipeline de classificação

Cada imagem nova passa por vários estágios:

1. **Classificação primária** — estratégia com tentativa principal e retry, prompts de fallback progressivos e recuperação por regex para JSON malformado
2. **Enriquecimento de anime** — condicional ao tipo de mídia; identifica série, personagens e nomes canônicos
3. **OCR** — Surya OCR (venv Python isolado) com fallback para o modelo de visão; roda para documentos, capturas e imagens com texto
4. **Extração de documentos** — regex + LLM extraem datas, valores e IDs de transação do texto do OCR

### Busca

Busca full-text em nomes de arquivo, caminhos, descrições, texto de OCR e nomes de personagens/séries. Exemplos:

- `anime ranma`
- `transferencia bancaria 2024`
- `recibo santander`
- `screenshot confidence >= 0.8`
- `face:alice` ou `face:"Nome Completo"`

### Publicação e fotos ocultas

- **Publicação:** o desktop gera um snapshot imutável do banco com `VACUUM INTO`, valida com `quick_check`, copia miniaturas e crops de forma incremental, grava `current.json` por último (tmp + fsync + rename) e poda revisões antigas. O cliente web lê apenas esse snapshot em modo read-only.
- **Fotos ocultas:** a coluna `files.hidden_at` marca linhas ocultas sem apagá-las. O desktop mostra o botão de olho no tile, o item no menu de contexto e o filtro "mostrar ocultas". O web nunca recebe linhas ocultas: busca, miniaturas, mídia, crops e metadados respondem `404` para elas, porque IDs são enumeráveis.

## Estrutura do projeto

```
sherlock/                  <- Aplicação principal
  core/                    <- Crate Rust de leitura compartilhado com o servidor web
                              (erro, parser de consultas, modelos, db, fs, similaridade)
  desktop/
    src-tauri/src/         <- Backend Rust do desktop
      classify.rs          <- Pipeline de visão no Ollama + OCR
      vision_prep.rs       <- Normalização/downscale das imagens enviadas à visão
      vision_estimate.rs   <- Projeção de tempo por pasta
      scan.rs              <- Scanner incremental com descoberta, pipeline, cancelamento e resume
      thumbnail.rs         <- Miniaturas, dHash e cache por raiz
      face.rs              <- Detecção (SCRFD) e reconhecimento (ArcFace) com ONNX
      publish.rs           <- Snapshot imutável, revisões e ponteiro atômico
      pdf.rs               <- Extração de texto com PDFium + renderização de páginas
      video.rs             <- Metadados via ffmpeg, keyframes e legendas
      video_server.rs      <- Streaming HTTP local com Range para preview de vídeo
      runtime.rs           <- Status do Ollama/GPU
      platform/            <- Abstração de SO (clipboard, GPU, caminhos Python)
    src/                   <- Frontend React do desktop
    src-tauri/scripts/     <- Scripts nativos (Surya OCR, PDFium, ONNX Runtime)
  shared/                  <- Pacote npm compartilhado pelos dois frontends:
                              tipos de leitura, utils puros, `LibraryClient`, `useLibrary()`
                              e as views React/CSS usadas nos dois apps
  web/
    server/                <- API axum somente leitura + SPA embutida
    ui/                    <- Cliente web Vite + React (PWA)
    Dockerfile             <- Imagem multi-stage
    docker-compose.yml     <- Compose de referência
_classification/           <- PoC em Python do pipeline de classificação
_bench_vision/             <- Benchmarks do pipeline de visão
_research_ab_test/         <- Pesquisa de A/B testing que informou a escolha de modelos
plans/                     <- Planos e notas de projeto (velocidade, web client, shares, etc.)
releases/                  <- Notas de release
docs/                      <- Documentação operacional
```

### Pesquisa e prototipagem (histórico)

Estes diretórios contêm o A/B testing que informou a escolha de modelos e o desenho do pipeline. Eles não fazem parte da aplicação principal.

```
_research_ab_test/
  scripts/                 <- Scripts de benchmark A/B
  docs/                    <- Notas de pesquisa (IDEA.md, RESULTS.md, etc.)
  lib/                     <- Helpers Python compartilhados
  results/                 <- Saídas geradas (gitignored)
  test_files/              <- Corpus de teste (gitignored, veja a nota abaixo)
```

> **Nota:** os arquivos de teste (imagens, áudio, vídeo, documentos) usados nos benchmarks não estão incluídos. Eles continham mídia com direitos autorais e documentos pessoais. Para repetir os benchmarks:
>
> 1. Adicione seus próprios arquivos em `_research_ab_test/test_files/` com subdiretórios como `images/`, `old_audio/`, `old_docs/`, `old_tvseries/`
> 2. Atualize os arquivos de ground truth em `_research_ab_test/docs/` para o seu corpus
> 3. Ajuste os scripts de benchmark conforme necessário

Os resultados (`_research_ab_test/docs/RESULTS.md`) mostram por que `qwen2.5vl:7b` foi escolhido em vez de `llava:13b` e `minicpm-v:8b` (80% de acurácia de tipo contra 33–50%), e por que Surya foi escolhido como OCR primário (95% de similaridade de referência, cobertura melhor que o modelo de visão sozinho).

## Armazenamento de dados

Todos os dados da aplicação ficam em `~/.local/share/frank_sherlock/`:

```
db/index.sqlite            <- Banco SQLite com FTS5
cache/thumbnails/          <- Miniaturas geradas (estrutura espelhada por raiz)
cache/classifications/     <- Cache de classificações
cache/tmp/                 <- Arquivos temporários (frames de GIF, etc.)
surya_venv/                <- Venv Python isolado para o Surya OCR
```

Os diretórios de origem nunca são modificados. O Frank Sherlock é estritamente somente leitura, com uma única exceção autorizada pelo usuário: a conversão opt-in de HEIC/NEF grava cópias JPEG em `FrankSherlock-JPG/` dentro do diretório do original.

A publicação do cliente web fica em uma pasta separada escolhida em Settings → Publicação:

```
current.json                         <- ponteiro da revisão ativa (único arquivo mutável)
index.<revisão>.sqlite               <- snapshot imutável, read-only
thumbs/<root_key>/<rel_path>.jpg     <- miniaturas publicadas
face_crops/<id>.jpg                  <- crops de rostos publicados
```

## CI e releases

Os workflows do repositório de origem continuam no repositório, mas este fork **não publica binários** e **não usa o updater do upstream**. O botão "Check for Updates" fica oculto para evitar que uma atualização do repositório de origem sobrescreva as mudanças locais. Se este fork passar a publicar releases próprios, basta trocar a constante `SHOW_UPDATE_CONTROLS` em `sherlock/desktop/src/App.tsx` e apontar o updater para o novo feed.

## Licença

Este projeto está licenciado sob a [GNU General Public License v3.0](LICENSE).

---

# Frank Sherlock (English)

Local-only, AI-powered image cataloging and search for your NAS. Point it at a directory, and it classifies every image using a local vision LLM, extracts text via OCR, generates thumbnails, and indexes everything into a searchable SQLite database. Nothing leaves your machine.

## What it does

- Scans image, PDF, and video directories read-only (JPEG, PNG, GIF, BMP, WebP, TIFF, PDF, MP4, MKV, AVI, MOV, WebM, and more)
- Converts HEIC and NEF photos into JPEG copies on request (`FrankSherlock-JPG/` next to the originals), so phone photos and camera raw files become searchable without changing the vision pipeline — the originals are never modified
- Classifies each image with a local Ollama vision model selected for your hardware (media type, description, anime/manga identification, document detection)
- Extracts text via Surya OCR (with vision LLM fallback) for documents, receipts, screenshots
- Generates 300px JPEG thumbnails for fast browsing (PDF 2-page montage)
- Full-text search with SQLite FTS5
- Finds duplicate files — exact matches by content fingerprint, near-duplicates by perceptual hash (dHash) + description similarity
- Detects and recognizes faces using native ONNX models (SCRFD + ArcFace) — clusters faces into people, lets you name and merge them, and search by `face:name`
- Detects renamed/moved files by fingerprint, so they don't get re-classified
- Resumes interrupted scans from the last checkpoint
- Live discovery progress during file walks on large directories
- Browse subdirectories via a collapsible tree in the sidebar
- "Refresh Metadata" rescans without Ollama (thumbnails only, no LLM)
- HEIC/NEF conversion is opt-in and reversible: the copies are ordinary JPEGs, tagged with their origin in the grid and skipped as automatic keepers in the duplicate view
- Hides photos from the library (desktop only) with a "show hidden" filter
- Publishes immutable index snapshots for a read-only web client, with revisions and an atomic pointer
- Opens the library in a browser or on a phone as a PWA, with token login and image, video (Range) and PDF preview
- Lets you pick the model, resize filter, description language, OCR engine and resume behavior in a Settings screen
- Offers a thumbnail repair command and database compaction for large libraries on network shares
- Shows live logs during a scan and an estimated ETA for the classification queue

## Screenshot

![Frank Sherlock screenshot](docs/frank_sherlock.png)

The UI is loosely inspired by VS Code: custom titlebar, collapsible sidebar, thumbnail grid, media type filters, preview overlay, duplicate finder, and automatic light/dark theme.

## Installing

This fork **does not publish binaries**. The upstream repository's releases do not include the changes described above. Building from source is the recommended path (see "Building from source").

You also need [Ollama](https://ollama.com) installed and running (`ollama serve`). On first launch, the app prompts you to download the vision model if it isn't installed yet.

### Windows: SmartScreen warning

The upstream MSI installer is not signed with a code-signing certificate. Windows SmartScreen may block it with a "Windows protected your PC" warning. Click "More info" and then "Run anyway" to proceed. The same applies to any local build of this fork.

## Requirements (for building from source)

- Linux, macOS, or Windows
- [Ollama](https://ollama.com) installed and running (`ollama serve`)
- NVIDIA GPU recommended on Linux/Windows (RTX series works well with `qwen2.5vl:7b`); Apple Silicon works natively on macOS
- A GPU is optional: on modest hardware `qwen2.5vl:3b` is the recommended choice, and Ollama can also run on CPU (slower)
- Node.js 20+
- Rust 1.77+

## Ollama model override

Frank Sherlock creates this commented runtime settings file on first startup:

- Linux: `~/.config/frank_sherlock/settings.toml`
- macOS: `~/Library/Application Support/frank_sherlock/settings.toml`
- Windows: `%APPDATA%\frank_sherlock\settings.toml`

By default the app chooses a vision model from detected hardware. To force a smaller or alternate model, edit the file (changes apply on the next scan):

```toml
model_override = "qwen2.5vl:3b"
```

The same file also controls how images are sent to the model:

```toml
# Longest side in pixels; 0 sends the original file untouched.
# Lower is much faster because Ollama's image preprocessing (not the model)
# dominates the cost of large photos. 1024 keeps small subjects and text legible.
image_max_side = 1024

# "concise" (default) or "detailed" (adds anime series/character candidates).
prompt_profile = "concise"

# Language of the description (what search matches on): "pt_br" (default) or "en".
# Only free text follows this: media types, enum values, OCR text and the
# "[Doc: …]" style markers stay English. Rows already described keep their
# language — "Convert existing descriptions" in the settings screen rewrites them
# with a text-only pass (minutes, no image is read).
description_language = "pt_br"

# OCR for screenshots/documents: "off", "llm" (default, fast) or "surya"
# (better recall, but spawns a Python process per image).
ocr_engine = "llm"

# What to do when opening a folder that is already indexed:
# "ask" (default), "classify_only" (resume pending work without re-walking the
# tree), "full" or "none".
startup_scan = "ask"
```

When a folder is opened with a CLI argument and it is already indexed, the app never re-scans on its own: it asks whether to resume the pending classification, verify changes (which walks the tree again — slow on network shares) or just open the existing index. The "Continue classification" path skips discovery, thumbnails and cleanup entirely, so nothing on disk is touched and nothing can be deleted.

All of these are also editable from the app: open the model & hardware status bar, click "Classification settings". The same dialog shows the model list with what fits in the current GPU, and — right before a scan starts — a time projection for that folder measured on your own machine (a few real images are classified to calibrate it; the result is cached).

When `model_override` is set, setup requires that exact Ollama model. If it is missing, the first-run setup dialog offers to download it with Ollama before scans can start. The first classification request loads the model on demand, and the app unloads Ollama models after scans finish.

Recommended override values:

- `qwen2.5vl:3b`: faster and safer for low VRAM, Windows NVIDIA <= 8GB, or CPU fallback.
- `qwen2.5vl:7b`: better quality when Ollama can load it fully on GPU.
- `qwen2.5vl:32b`: heavy; only realistic for large unified-memory systems.

Experimental alternatives such as `minicpm-v:8b` or `moondream` may work if installed in Ollama, but the prompts are tuned for `qwen2.5vl`.

## Network shares and NAS paths

Roots are indexed by path, so shares deserve care. A share that is unmounted at startup, after suspend or after a logout is no longer purged: it is kept and flagged `Offline` in the sidebar, and `Locate…` repoints the existing root at a new mount path while keeping classification, albums, duplicates and faces. Scans refuse to run when discovery collapses to zero files instead of deleting the index.

See [docs/network-shares-linux.md](docs/network-shares-linux.md) for systemd automount setup, GVFS caveats, relinking and database compaction.

## Web client (read-only)

The web app is a read-only view of a library published by the desktop. The desktop still owns the pipeline; the container only reads an immutable snapshot.

1. **Publish from the desktop:** Settings → **Publicação**. Pick a folder on the network share (`publish_dir`) that is not inside a photo root. Keep "Publish after every scan" on, and pick how many revisions to keep (default 2).
2. **Configure the media map:** copy `config/media.map.example.json` to `config/media.map.json` and map each `storedPath` (as the desktop recorded it) to a `mountedPath` inside the container.
3. **Start the container:**

```bash
cd sherlock/web
cp .env.example .env       # set AUTH_TOKEN and the host paths inside
docker compose up -d --build
```

4. **Sign in from the browser:** open the service URL and enter the same `AUTH_TOKEN` in the login screen. The token is stored in the device's `localStorage`. A link with `?token=...` still works for the first visit and is removed from the address bar after it is stored.

The web client includes search with filters and sorting, an infinite-scroll grid, a sidebar with roots, subdirectories, albums and smart folders, face browsing, informative duplicates, image/video/PDF preview, a responsive layout with a drawer, a PWA manifest and an app-shell service worker. The API is always network-only; there is no offline library.

The app token is a second barrier, not the primary one: put a reverse proxy in front (Caddy, Traefik, nginx) with Basic Auth or Authentik and TLS. Without it, any device on the LAN can read the whole library. An empty `AUTH_TOKEN` disables the check and logs a `WARN`; use it for debugging only.

See [docs/web-deploy.md](docs/web-deploy.md) for the full walkthrough, including SMB/CIFS, GVFS, 401/503 troubleshooting and installing on a phone.

## Building from source

For a full Ubuntu walkthrough (Rust, system libraries, Node, Ollama), see [docs/dev-setup-linux.md](docs/dev-setup-linux.md). To produce an installable `.deb` or AppImage, see [docs/build-and-install-linux.md](docs/build-and-install-linux.md).

```bash
# 1. Install the workspace dependencies (from the repository root)
npm install

# 2. Download native libraries for your platform
cd sherlock/desktop/src-tauri
bash scripts/download-pdfium.sh
bash scripts/download-onnxruntime.sh
cd ../../..

# 3. Start Ollama (in a separate terminal)
ollama serve

# 4. Run in dev mode
cd sherlock/desktop
npm run tauri:dev
```

To produce a release binary (AppImage/DMG/MSI):

```bash
cd sherlock/desktop
npm run tauri:build
```

Output will be in `sherlock/desktop/src-tauri/target/release/bundle/`.

### Wayland/NVIDIA workaround

If the WebKit window is blank on Wayland with NVIDIA drivers:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 GDK_BACKEND=wayland,x11 npm run tauri:dev
```

## Tests

```bash
# Shared frontend
npm run test:shared

# Web client
npm run test:web

# Desktop frontend
npm run test:desktop

# Desktop Rust backend
cd sherlock/desktop && npm run test:rust

# Shared read core
cd sherlock/core && cargo test
```

The suite covers classification JSON parsing, thumbnail generation, incremental scanning, database operations (upsert, touch, delete, FTS), scan cancellation, query parsing, duplicate detection, similarity scoring, video metadata, face detection/clustering, snapshot publishing, the web client's HTTP contract, and the UI components of both frontends.

## How it works

### Incremental scanning

Built for large NAS directories with 100k+ files:

1. **Discovery** — walks the directory tree using only filesystem metadata (mtime, size) with zero file reads. WalkDir metadata is reused to skip redundant stat syscalls. Child root subtrees are skipped early via `filter_entry()`. Progress is reported live to the UI every 500 files.
2. **Thumbnailing and pipeline** — new and modified files are fingerprinted, thumbnailed, and inserted into the DB with minimal records (confidence=0). Moved files are detected by fingerprint and just update their path reference. Unchanged file markers are flushed in batch before the heavy work starts.
3. **Classifying** — unclassified files are sent to Ollama for LLM classification and their records are updated with rich metadata. This phase can be skipped entirely via "Refresh Metadata" for fast, Ollama-free rescans.
4. **Cleanup** — files no longer on disk are soft-deleted, and their cached thumbnails are removed.

Each phase supports cancel and resume independently, with progress checkpointed after every file. Rescanning an unchanged 10k-image directory takes seconds.

### Optimizations in this fork for large libraries

- **One decode, two artifacts:** the 300px thumbnail and the 1024px JPEG used by vision come from the same decode and the same file read. Each photo used to be read and decoded twice, which is especially expensive over SMB.
- **CPU/GPU pipeline:** in backlog mode, a CPU producer prepares the next file while Ollama occupies the GPU, reducing idle time on both sides.
- **Cheaper vision request:** downscale before sending, image-derived `num_ctx`, a short prompt with `format: json`, a lower `num_predict`, and an OCR gate that avoids running Surya on every image that contains text.
- **dHash deduplication:** visually identical photos (within a configurable Hamming distance) inherit the representative's classification; identical thumbnails are not decoded again and are shared via hardlink.
- **Per-root thumbnail cache:** `cache/thumbnails/<root_key>/<rel_path>.jpg`, with an idempotent boot migration, validation that does not depend on the SMB mtime, and self-healing of dead `thumb_path` values.
- **Repair and compaction:** the "Repair Thumbnails" command regenerates only what is missing, with progress and cancellation; "Compact DB" reclaims space with VACUUM.
- **Configurable resize filter:** box for JPEG/HEIC (faster) or Lanczos3 for PNG/BMP/TIFF, with quality as the trade-off.

### Duplicate detection

Find and remove redundant copies to reclaim disk space:

- **Exact duplicates** — groups files with identical SHA-256 fingerprints. A keeper heuristic picks the oldest file with the shortest path.
- **Near-duplicates** — perceptual similarity using dHash (computed during thumbnail generation), combined with Jaccard word overlap on LLM descriptions (85% visual + 15% textual). Uses Union-Find for transitive grouping.
- **Group comparison** — side-by-side preview of all files in a duplicate group with per-file metadata.
- **3-tier confidence coloring** — green (exact, safe to delete), yellow (near >= 85%), red (lower, needs visual check).

### Face detection and recognition

Find and organize people across your photo library:

1. **Detection** — SCRFD (ONNX) finds faces with bounding boxes and 5-point keypoints. Models are downloaded automatically on first use from the InsightFace buffalo_l pack.
2. **Recognition** — ArcFace produces 512-dimensional embeddings for each detected face, enabling identity-aware comparison.
3. **Clustering** — cosine similarity (threshold 0.45) groups faces into people. New faces are assigned to existing clusters or start new ones.
4. **Management** — name people, merge duplicates, remove false detections. FacesView shows all people with representative face crops.
5. **Search** — `face:alice`, `face:"Full Name"`, or `face:42` (by person ID) to find all photos of someone.

Face detection runs per-folder from the context menu, shows progress in the status bar, and can also run inside the scan pass.

### Classification pipeline

Each new image goes through several stages:

1. **Primary classification** — a primary attempt plus retry, progressive fallback prompts, and regex salvage for malformed JSON
2. **Anime enrichment** — conditional on media type; identifies series, characters, and canonical names
3. **OCR** — Surya OCR (isolated Python venv) with vision LLM fallback; runs for documents, screenshots, and text-containing images
4. **Document extraction** — regex + LLM extraction of dates, amounts, transaction IDs from OCR text

### Search

Full-text search across filenames, paths, descriptions, OCR text, and character/series names. Example queries:

- `anime ranma`
- `bank transfer 2024`
- `receipt santander`
- `screenshot confidence >= 0.8`
- `face:alice` or `face:"Full Name"`

### Publishing and hidden photos

- **Publishing:** the desktop produces an immutable database snapshot with `VACUUM INTO`, validates it with `quick_check`, copies thumbnails and crops incrementally, writes `current.json` last (tmp + fsync + rename), and prunes old revisions. The web client reads only that snapshot in read-only mode.
- **Hidden photos:** the `files.hidden_at` column marks hidden rows without deleting them. The desktop shows the eye button on the tile, the context-menu item, and the "show hidden" filter. The web never receives hidden rows: search, thumbnails, media, crops and metadata answer `404` for them, because IDs are enumerable.

## Project structure

```
sherlock/                  <- Main application
  core/                    <- Rust read core shared with the web server
                              (error, query parser, models, db, fs, similarity)
  desktop/
    src-tauri/src/         <- Rust desktop backend
      classify.rs          <- Ollama vision + OCR pipeline
      vision_prep.rs       <- Normalization/downscale of images sent to vision
      vision_estimate.rs   <- Per-folder time projection
      scan.rs              <- Incremental scanner with discovery, pipeline, cancel and resume
      thumbnail.rs         <- Thumbnails, dHash and per-root cache
      face.rs              <- Native ONNX face detection (SCRFD) + recognition (ArcFace)
      publish.rs           <- Immutable snapshot, revisions and atomic pointer
      pdf.rs               <- PDFium text extraction + page rendering
      video.rs             <- ffmpeg metadata, keyframes, subtitle parsing
      video_server.rs      <- Localhost HTTP Range streaming for video preview
      runtime.rs           <- Ollama/GPU status
      platform/            <- OS abstraction (clipboard, GPU, Python paths)
    src/                   <- React desktop frontend
    src-tauri/scripts/     <- Native scripts (Surya OCR, PDFium, ONNX Runtime)
  shared/                  <- npm package shared by both frontends:
                              read types, pure utils, `LibraryClient`, `useLibrary()`
                              and the React/CSS views used by both apps
  web/
    server/                <- Read-only axum API + embedded SPA
    ui/                    <- Vite + React web client (PWA)
    Dockerfile             <- Multi-stage image
    docker-compose.yml     <- Reference compose
_classification/           <- Python PoC of the classification pipeline
_bench_vision/             <- Vision pipeline benchmarks
_research_ab_test/         <- A/B research that informed model selection
plans/                     <- Design notes (speed, web client, shares, etc.)
releases/                  <- Release notes
docs/                      <- Operational documentation
```

### Research and prototyping (historical)

These directories contain the A/B testing research that informed model selection and pipeline design. They aren't part of the main application.

```
_research_ab_test/
  scripts/                 <- A/B benchmark scripts
  docs/                    <- Research notes (IDEA.md, RESULTS.md, etc.)
  lib/                     <- Shared Python helpers
  results/                 <- Generated benchmark outputs (gitignored)
  test_files/              <- Test corpus (gitignored, see note below)
```

> **Note:** The test files (images, audio, video, documents) used for benchmarking are not included. They contained copyrighted media and personal documents. To re-run the benchmarks:
>
> 1. Add your own media files in `_research_ab_test/test_files/` with subdirectories like `images/`, `old_audio/`, `old_docs/`, `old_tvseries/`
> 2. Update the ground truth JSON files in `_research_ab_test/docs/` to match your corpus
> 3. Adjust the benchmark scripts as needed

The benchmark results (`_research_ab_test/docs/RESULTS.md`) show why `qwen2.5vl:7b` was chosen over `llava:13b` and `minicpm-v:8b` (80% type accuracy vs 33-50%), and why Surya was picked as primary OCR (95% reference similarity, better coverage than vision LLM alone).

## Data storage

All application data lives under `~/.local/share/frank_sherlock/`:

```
db/index.sqlite            <- SQLite database with FTS5
cache/thumbnails/          <- Generated thumbnails (mirrored per-root path structure)
cache/classifications/     <- Classification cache
cache/tmp/                 <- Temporary files (GIF frames, etc.)
surya_venv/                <- Isolated Python venv for Surya OCR
```

Source directories are never modified. Frank Sherlock is strictly read-only, with one user-authorized exception: opt-in HEIC/NEF conversion writes JPEG copies into `FrankSherlock-JPG/` inside the original's directory.

The web client publication lives in a separate folder chosen in Settings → Publicação:

```
current.json                         <- active revision pointer (the only mutable file)
index.<revision>.sqlite              <- immutable, read-only snapshot
thumbs/<root_key>/<rel_path>.jpg     <- published thumbnails
face_crops/<id>.jpg                  <- published face crops
```

## CI and releases

The upstream repository's workflows remain in the repository, but this fork **does not publish binaries** and **does not use the upstream updater**. The "Check for Updates" button is hidden so an upstream update cannot overwrite local changes. If this fork ever publishes its own releases, flip the `SHOW_UPDATE_CONTROLS` constant in `sherlock/desktop/src/App.tsx` and point the updater at the new feed.

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
