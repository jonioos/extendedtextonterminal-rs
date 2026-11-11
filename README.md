# ETOT — Jonio Extended Text On Terminal

**Editor di testo per jonioOS.** ETOT è un editor per terminale scritto in Rust, con interfaccia colorata, strumenti per il codice e comandi **VIM COMPATIBLE**. La compatibilità è in sviluppo.

## Installazione

```sh
cargo build --release
./scripts/install-etot
etot --edit file.txt
etot --read file.txt
```

Lo script installa il binario in `~/usr/bin/etot`. Si può scegliere un altro percorso assoluto con `ETOT_INSTALL_PATH`.

## Funzioni disponibili

- Modalità Normal, Insert, Visual, Visual Line, Visual Block, Replace e Command. In Insert il cursore diventa una barra; tornando in Normal riacquista la forma a blocco. Canc elimina la selezione Visual, in Normal equivale a `x` e in Insert elimina il carattere successivo.
- Movimenti, operatori, conteggi, registri, macro, undo/redo, ricerca incrementale `/` e `?`, sostituzione `:s`/`:%s` e comandi Ex di base.
- `$end` salva il file e chiude l'editor; `$quit` chiude senza salvare. Si possono digitare direttamente in Normal oppure nella Cmdline. `$` continua a spostare il cursore a fine riga.
- `$undo` e `$redo` usano la cronologia delle modifiche. Le modifiche non salvate e l'undo dei file nominati sono conservati in `~/.local/state/etot` e ripristinati all'apertura se il file su disco non è cambiato. `$quit` scarta il recupero delle modifiche non salvate.
- Buffer, finestre divise, tab, esploratore, fuzzy finder, ricerca nel progetto, Git status/diff/staging e terminale integrato.
- Parsing ed evidenziazione Tree-sitter per Rust, TypeScript, JavaScript, Python, Go, C, C++, C#, VB.NET, HTML, CSS, JSON, YAML, TOML, Bash, Lua e Markdown.
- Client LSP con completamento, diagnostica, definizione, riferimenti, hover, formattazione, simboli, code action e rename. I server esterni vanno installati separatamente: `rust-analyzer`, `typescript-language-server`, `pyright-langserver`, `gopls`, `clangd`, `vscode-html-language-server`, `vscode-css-language-server`, `csharp-ls` e `vbnet-ls`.
- `:Format` e `:Lint` avviano strumenti esterni in background; i diagnostici riconosciuti sono mostrati nella quickfix. Sono preconfigurati rustfmt/cargo, ruff, gofmt/go vet, clang-format/clang-tidy, prettier ed eslint dove applicabili. `:SessionSave` e `:SessionLoad` conservano buffer, tab e split; `:terminal` apre una shell PTY in un pannello inferiore, chiudibile con `Ctrl+\`. I registri `"+` e `"*` accedono alla clipboard di sistema.

La configurazione è in `~/.config/etot/config.toml`; vedere [config.toml.example](config.toml.example). Per mantenere reattivi i file grandi, l'evidenziazione completa viene omessa oltre 256 KiB. Lo stato della compatibilità e i limiti attuali sono in [docs/stato.md](docs/stato.md).

## Verifica

```sh
cargo test
cargo check -q
```
