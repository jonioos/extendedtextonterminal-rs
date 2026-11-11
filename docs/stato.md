# Stato ETOT-RS

Aggiornato l'11 novembre 2026. `target/release/etot` è un eseguibile Rust autonomo; `/usr/bin/etot` usa ancora la precedente implementazione.

## Implementato

- Editor TUI con modalità Vim principali, buffer, split, tab, registri nominati e speciali `0`/`1–9`/`-`/`_`, macro, undo/redo, ricerca incrementale con frecce, operatori, text object anche tra righe, changelist `g;`/`g,`, `Ctrl+O` temporaneo in Insert e navigazione di base.
- Comandi Ex per file, buffer, sostituzione con range e flag, ordinamento, copia, spostamento, cancellazione, incolla, unione righe, lettura di file e `:global`/`:vglobal` per stampa, copia, cancellazione e sostituzione.
- Tema a colori, etichette file compatibili con MesloLGS, statusline, popup del comando con suggerimenti subito sotto, completamento e suggerimenti contestuali.
- Tree-sitter integrato anche per Go, C# e VB.NET; adattatori LSP preconfigurati per Python, Go, C/C++, HTML/CSS, C# e VB.NET (server esterni richiesti); explorer, fuzzy finder, quickfix, Git e shell.
- Avvio LSP in background, sincronizzazione differita, evidenziazione differita e limitata a 256 KiB, ricerca con memoria costante, undo limitato a 32 MiB e dimensioni terminale tramite ioctl.
- Salvataggio atomico, controllo delle modifiche esterne, modalità read-only, configurazione TOML e opzioni `ignorecase`/`smartcase`, `autoindent` e `shiftwidth`.
- `$undo`/`$redo`, registri clipboard `+`/`*`, journal atomico per recupero e undo persistente dei file nominati, sessioni con buffer/tab/split, `:Format`/`:Lint` con tool esterni configurabili e terminale PTY in pannello.

## Mancante

- Compatibilità Vimscript completa: linguaggio, funzioni, autocmd, mappe, API e plugin Vimscript non sono implementati. Mancano anche regex Vim completa, registri speciali, Ex avanzato e semantica degli operatori nei casi limite.
- Snippet, LSP avanzato e code action multifile, debugger e sistema plugin. C#/VB.NET usano ancora la formattazione LSP, senza formatter/linter esterni predefiniti.
- Buffer efficiente per file molto grandi, recovery dei file cambiati esternamente, copertura Unicode e test di compatibilità estesi.

La compatibilità VIM completa e gli strumenti IDE avanzati richiedono ancora lavoro.
