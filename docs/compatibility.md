# Compatibilità VIM di ETOT

Questa tabella registra i comandi VIM COMPATIBLE già disponibili e i comportamenti ancora da completare.

| Area | Comportamento di riferimento | Stato etot-rs |
| --- | --- | --- |
| Modalità | Normal, Insert, Visual, Replace, Ex, ricerca | Tutte avviate; Visual e Replace hanno ancora semantica parziale |
| Ricerca | `/` avanti, `?` indietro, `n/N`, `*`/`#`, ricerca incrementale | Regex Rust, anteprima incrementale con Esc che ripristina posizione e pattern, ritorno circolare, frecce dopo `?`, `*`/`#`; mancano offset e sintassi regex Vim |
| Movimento | Parole, frasi, paragrafi, text objects, conteggi | `h/j/k/l`, `w/W/b/B/e/E`, `f/F/t/T`, `;/,`, `%` con parentesi annidate, `{}` su righe vuote, `H/M/L` nella finestra visibile, `0/^/$`, `gg/G`, `Ctrl+D/U/F/B`, frecce e conteggi; frasi e altri movimenti mancano |
| Modifica | Operatori, registri, put, ripetizione, undo/redo | `d/c/y` con subset di movimenti e oggetti per parole/virgolette/parentesi sulla stessa riga, `>>/<<` con conteggi e in Visual, registri nominati, macro per buffer e `.` di base, Visual e Replace; marks e jump list locali di base, `:set` parziale; mancano registri speciali, macro tra buffer, `>`/`<` con movimenti e semantica completa di `.` |
| File | Buffer, finestre, tab, Ex | Più buffer, split con `:split/:vsplit`, tab con `:tabnew/:tabnext/:tabprevious/:tabclose` e `gt/gT`, comandi `:e/:bn/:bp/:buffers/:bd/:wa`; molti Ex mancano |
| LazyVim | Picker, LSP, completamento, Git, parsing, plugin | Explorer, picker, grep, Tree-sitter con parsing e colori di base per le 14 famiglie richieste, LSP con formattazione, simboli, code action locali e rename testuale multifile, Git status/diff/staging, shell a schermo intero; esperienza ancora incompleta |
| Terminale | TUI, Unicode, resize, mouse, clipboard | TUI ANSI, larghezza Unicode di base, scrolling orizzontale per finestra, alternanza schermo, SIGWINCH e ripristino su SIGTERM/SIGHUP/SIGINT; mouse e clipboard mancanti |

La configurazione TOML copre tema, numeri di riga, keymap normali a un carattere e server LSP. Il salvataggio preserva i permessi, usa file temporaneo e sostituzione atomica e blocca la scrittura se il file cambia su disco. Restano undo persistente, recovery e gestione completa di symlink e formati di testo.
