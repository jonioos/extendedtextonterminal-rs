# Compatibilità VIM di ETOT

Questa tabella registra i comandi VIM COMPATIBLE già disponibili e i comportamenti ancora da completare.

| Area | Comportamento di riferimento | Stato etot-rs |
| --- | --- | --- |
| Modalità | Normal, Insert, Visual, Replace, Ex, ricerca | Tutte avviate; Visual e Replace hanno ancora semantica parziale |
| Ricerca | `/` avanti, `?` indietro, `n/N`, `*`/`#`, ricerca incrementale | Regex Rust, anteprima incrementale con Esc che ripristina posizione e pattern, ritorno circolare, frecce dopo `?`, `*`/`#`; mancano offset e sintassi regex Vim |
| Movimento | Parole, frasi, paragrafi, text objects, conteggi | `h/j/k/l`, `w/W/b/B/e/E`, `f/F/t/T`, `;/,`, `%` con parentesi annidate, `{}` su righe vuote, `H/M/L` nella finestra visibile, `0/^/$`, `gg/G`, `Ctrl+D/U/F/B`, frecce e conteggi; frasi e altri movimenti mancano |
| Modifica | Operatori, registri, put, ripetizione, undo/redo | `d/c/y` con subset di movimenti e oggetti per parole/virgolette/parentesi sulla stessa riga, `>>/<<` con conteggi e in Visual, registri nominati e clipboard `+/*`, macro per buffer e `.` di base, Visual e Replace; marks e jump list locali di base, `:set` parziale, `$undo`/`$redo`; mancano registri speciali, macro tra buffer, `>`/`<` con movimenti e semantica completa di `.` |
| File | Buffer, finestre, tab, Ex | Più buffer, split con `:split/:vsplit`, tab con `:tabnew/:tabnext/:tabprevious/:tabclose` e `gt/gT`, comandi `:e/:bn/:bp/:buffers/:bd/:wa`; molti Ex mancano |
| Ambiente IDE | Picker, LSP, completamento, Git, parsing, strumenti | Explorer, picker, grep, Tree-sitter con parsing e colori di base per le 14 famiglie richieste, LSP, Git status/diff/staging, formatter e linter esterni configurabili, sessioni manuali e terminale PTY in pannello; esperienza ancora incompleta |
| Terminale | TUI, Unicode, resize, mouse, clipboard | TUI ANSI, larghezza Unicode di base, scrolling orizzontale per finestra, alternanza schermo, SIGWINCH, clipboard `+/*` e ripristino su SIGTERM/SIGHUP/SIGINT; mouse mancante |
| Vimscript | Script e plugin Vim | Interprete Vimscript non presente; `:source`, funzioni, autocmd, mapping e semantica completa delle espressioni mancano |

La configurazione TOML copre tema, numeri di riga, keymap normali a un carattere, server LSP e strumenti esterni. Il salvataggio preserva i permessi, usa file temporaneo e sostituzione atomica e blocca la scrittura se il file cambia su disco. I file di stato ripristinano modifiche non salvate e undo dei file nominati quando il file su disco non è cambiato; restano la gestione completa di symlink e formati di testo.
