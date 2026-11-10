//! Parser for composable normal-mode commands. It keeps pending prefixes
//! separate from the buffer so counts and operators share one grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Word,
    BigWord,
    BackWord,
    BigBackWord,
    WordEnd,
    BigWordEnd,
    Start,
    FirstNonBlank,
    End,
    FirstLine,
    LastLine,
    MatchBracket,
    ParagraphBackward,
    ParagraphForward,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    Find {
        target: char,
        forward: bool,
        till: bool,
        repeat: bool,
    },
}
impl Motion {
    pub fn repeated(self) -> Self {
        match self {
            Self::Find {
                target,
                forward,
                till,
                ..
            } => Self::Find {
                target,
                forward,
                till,
                repeat: true,
            },
            other => other,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Move(Motion, usize),
    GoLine(Option<usize>),
    Definition,
    References,
    Hover,
    Operate(Operator, Motion, usize),
    TextObject(Operator, bool, char, usize),
    Lines(Operator, usize),
    Insert,
    Append,
    InsertFirstNonBlank,
    AppendEnd,
    OpenBelow,
    OpenAbove,
    DeleteChar(usize),
    DeleteBefore(usize),
    Substitute(usize),
    JoinLines(usize),
    ToggleCase(usize),
    Indent(bool, usize),
    ShiftMotion(bool, Motion, usize),
    ShiftTextObject(bool, bool, char, usize),
    Put(bool, usize),
    Undo(usize),
    Redo(usize),
    Repeat(usize),
    SelectRegister(char),
    StartMacro(char),
    PlayMacro(char, usize),
    SetMark(char),
    JumpMark(char, bool),
    Search(bool),
    SearchWord(bool),
    Command,
    RepeatSearch(bool),
    ChangeList(bool, usize),
    ClearSearch,
    Visual(char),
    Replace(bool, usize),
}
#[derive(Debug, Clone, Copy)]
pub struct CommandHint {
    pub title: &'static str,
    pub choices: &'static [(&'static str, &'static str)],
}
#[derive(Debug, Default, Clone)]
pub struct CommandEngine {
    count: usize,
    after_operator: usize,
    operator: Option<Operator>,
    object_prefix: Option<bool>,
    prefix_g: bool,
    register_prefix: bool,
    macro_record_prefix: bool,
    macro_play_prefix: bool,
    mark_prefix: Option<char>,
    find_prefix: Option<(bool, bool)>,
    last_find: Option<(char, bool, bool)>,
    indent_prefix: Option<char>,
}
impl CommandEngine {
    pub fn hint(&self) -> Option<CommandHint> {
        let (title, choices): (_, &'static [(&str, &str)]) = if self.indent_prefix.is_some() {
            (
                "Indentazione",
                &[
                    ("> / <", "riga"),
                    ("j/k", "righe"),
                    ("i/a", "oggetto"),
                    ("G", "fine file"),
                ],
            )
        } else if self.find_prefix.is_some() {
            (
                "Trova carattere",
                &[("carattere", "cerca nella riga"), ("Esc", "annulla")],
            )
        } else if self.mark_prefix.is_some() {
            ("Marks", &[("a-z", "mark locale"), ("Esc", "annulla")])
        } else if self.macro_record_prefix {
            (
                "Registra macro",
                &[("a-z", "registro"), ("q", "termina dopo")],
            )
        } else if self.macro_play_prefix {
            (
                "Riproduci macro",
                &[("a-z", "registro"), ("@", "ultima macro")],
            )
        } else if self.register_prefix {
            (
                "Registro",
                &[
                    ("a-z", "seleziona"),
                    ("A-Z", "aggiungi"),
                    ("0-9 / - / _", "speciali"),
                    ("\"", "predefinito"),
                ],
            )
        } else if self.prefix_g {
            (
                "Vai a",
                &[
                    ("g", "prima riga"),
                    ("d", "definizione"),
                    ("r", "riferimenti"),
                    ("t/T", "scheda"),
                ],
            )
        } else if let Some(operator) = self.operator {
            if self.object_prefix.is_some() {
                (
                    "Oggetto di testo",
                    &[
                        ("w/W", "parola"),
                        ("\" / ' / `", "citazione"),
                        ("( [ {", "parentesi"),
                    ],
                )
            } else {
                match operator {
                    Operator::Delete => (
                        "Elimina",
                        &[
                            ("d", "righe"),
                            ("w/e", "parole"),
                            ("$", "fine riga"),
                            ("i/a", "oggetto"),
                            ("f/t", "carattere"),
                            ("%", "parentesi"),
                        ],
                    ),
                    Operator::Change => (
                        "Cambia",
                        &[
                            ("c", "righe"),
                            ("w/e", "parole"),
                            ("$", "fine riga"),
                            ("i/a", "oggetto"),
                            ("f/t", "carattere"),
                            ("%", "parentesi"),
                        ],
                    ),
                    Operator::Yank => (
                        "Copia",
                        &[
                            ("y", "righe"),
                            ("w/e", "parole"),
                            ("$", "fine riga"),
                            ("i/a", "oggetto"),
                            ("f/t", "carattere"),
                            ("%", "parentesi"),
                        ],
                    ),
                }
            }
        } else if self.count > 0 {
            (
                "Conteggio",
                &[
                    ("d/c/y", "operatore"),
                    ("j/k", "riga"),
                    ("w/b", "parola"),
                    ("G", "vai alla riga"),
                ],
            )
        } else {
            return None;
        };
        Some(CommandHint { title, choices })
    }
    pub fn operator_pending(&self) -> bool {
        self.operator.is_some() || self.indent_prefix.is_some()
    }
    pub fn has_pending(&self) -> bool {
        self.count > 0
            || self.operator.is_some()
            || self.prefix_g
            || self.register_prefix
            || self.macro_record_prefix
            || self.macro_play_prefix
            || self.mark_prefix.is_some()
            || self.find_prefix.is_some()
            || self.indent_prefix.is_some()
    }
    pub fn reset(&mut self) {
        let last_find = self.last_find;
        *self = Self::default();
        self.last_find = last_find;
    }
    fn take_count(&mut self) -> usize {
        let value = self.count.max(1).saturating_mul(self.after_operator.max(1));
        self.reset();
        value
    }
    pub fn feed(&mut self, key: char) -> Option<Action> {
        if let Some(prefix) = self.indent_prefix {
            if key.is_ascii_digit() && (key != '0' || self.after_operator > 0) {
                let value = key.to_digit(10).unwrap_or(0) as usize;
                self.after_operator = self
                    .after_operator
                    .saturating_mul(10)
                    .saturating_add(value)
                    .min(100_000);
                return None;
            }
            if let Some(inner) = self.object_prefix {
                let count = self.take_count();
                return matches!(key, 'p' | 'w' | 'W' | '(' | ')' | '[' | ']' | '{' | '}')
                    .then_some(Action::ShiftTextObject(prefix == '>', inner, key, count));
            }
            if key == 'i' || key == 'a' {
                self.object_prefix = Some(key == 'i');
                return None;
            }
            let count = self.take_count();
            return if key == prefix {
                Some(Action::Indent(prefix == '>', count))
            } else {
                motion(key).map(|motion| Action::ShiftMotion(prefix == '>', motion, count))
            };
        }
        if let Some((forward, till)) = self.find_prefix.take() {
            let op = self.operator;
            let count = self.take_count();
            self.last_find = Some((key, forward, till));
            let motion = Motion::Find {
                target: key,
                forward,
                till,
                repeat: false,
            };
            return Some(op.map_or(Action::Move(motion, count), |op| {
                Action::Operate(op, motion, count)
            }));
        }
        if let Some(prefix) = self.mark_prefix.take() {
            self.reset();
            return match prefix {
                'm' => Some(Action::SetMark(key)),
                '\'' => Some(Action::JumpMark(key, true)),
                '`' => Some(Action::JumpMark(key, false)),
                _ => None,
            };
        }
        if matches!(key, 'm' | '\'' | '`') && self.operator.is_none() {
            self.mark_prefix = Some(key);
            return None;
        }
        if self.macro_record_prefix {
            self.reset();
            return Some(Action::StartMacro(key));
        }
        if self.macro_play_prefix {
            return Some(Action::PlayMacro(key, self.take_count()));
        }
        if self.register_prefix {
            self.register_prefix = false;
            return Some(Action::SelectRegister(key));
        }
        if key == '"' && self.operator.is_none() {
            self.register_prefix = true;
            return None;
        }
        if key == 'q' && self.operator.is_none() {
            self.macro_record_prefix = true;
            return None;
        }
        if key == '@' && self.operator.is_none() {
            self.macro_play_prefix = true;
            return None;
        }
        if key.is_ascii_digit() && (key != '0' || self.count > 0 || self.after_operator > 0) {
            let value = key.to_digit(10)? as usize;
            let target = if self.operator.is_some() {
                &mut self.after_operator
            } else {
                &mut self.count
            };
            *target = target.saturating_mul(10).saturating_add(value).min(100_000);
            return None;
        }
        if matches!(key, '>' | '<') && self.operator.is_none() {
            self.indent_prefix = Some(key);
            return None;
        }
        if matches!(key, 'f' | 'F' | 't' | 'T') && !self.prefix_g && self.object_prefix.is_none() {
            self.find_prefix = Some((matches!(key, 'f' | 't'), matches!(key, 't' | 'T')));
            return None;
        }
        if matches!(key, ';' | ',') && !self.prefix_g && self.object_prefix.is_none() {
            let op = self.operator;
            let count = self.take_count();
            let (target, forward, till) = self.last_find?;
            let motion = Motion::Find {
                target,
                forward: if key == ';' { forward } else { !forward },
                till,
                repeat: true,
            };
            return Some(op.map_or(Action::Move(motion, count), |op| {
                Action::Operate(op, motion, count)
            }));
        }
        if self.prefix_g {
            if matches!(key, ';' | ',') {
                let count = self.take_count();
                return Some(Action::ChangeList(key == ',', count));
            }
            if key == 'g' {
                return Some(Action::Move(Motion::FirstLine, self.take_count()));
            }
            if key == 'd' {
                self.reset();
                return Some(Action::Definition);
            }
            if key == 'r' {
                self.reset();
                return Some(Action::References);
            }
            self.reset();
            return None;
        }
        if let Some(op) = self.operator {
            if let Some(inner) = self.object_prefix {
                let count = self.take_count();
                return matches!(
                    key,
                    'w' | 'W' | 'p' | '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}'
                )
                .then_some(Action::TextObject(op, inner, key, count));
            }
            if key == 'i' || key == 'a' {
                self.object_prefix = Some(key == 'i');
                return None;
            }
            let own = match op {
                Operator::Delete => 'd',
                Operator::Change => 'c',
                Operator::Yank => 'y',
            };
            if key == own {
                return Some(Action::Lines(op, self.take_count()));
            }
            if let Some(motion) = motion(key) {
                return Some(Action::Operate(op, motion, self.take_count()));
            }
            self.reset();
            return None;
        }
        if key == 'g' {
            self.prefix_g = true;
            return None;
        }
        if let Some(op) = operator(key) {
            self.operator = Some(op);
            return None;
        }
        let explicit_count = self.count > 0;
        let count = self.take_count();
        Some(match key {
            'K' => Action::Hover,
            'G' => Action::GoLine(explicit_count.then_some(count)),
            'v' => Action::Visual('v'),
            'V' => Action::Visual('V'),
            'R' => Action::Replace(false, count),
            'r' => Action::Replace(true, count),
            'i' => Action::Insert,
            'I' => Action::InsertFirstNonBlank,
            'a' => Action::Append,
            'A' => Action::AppendEnd,
            'o' => Action::OpenBelow,
            'O' => Action::OpenAbove,
            'x' => Action::DeleteChar(count),
            'X' => Action::DeleteBefore(count),
            's' => Action::Substitute(count),
            'S' => Action::Lines(Operator::Change, count),
            'D' => Action::Operate(Operator::Delete, Motion::End, 1),
            'C' => Action::Operate(Operator::Change, Motion::End, 1),
            'Y' => Action::Lines(Operator::Yank, count),
            'J' => Action::JoinLines(count),
            '~' => Action::ToggleCase(count),
            'p' => Action::Put(true, count),
            'P' => Action::Put(false, count),
            'u' => Action::Undo(count),
            '.' => Action::Repeat(count),
            '/' => Action::Search(true),
            '?' => Action::Search(false),
            '*' => Action::SearchWord(true),
            '#' => Action::SearchWord(false),
            ':' => Action::Command,
            'n' => Action::RepeatSearch(true),
            'N' => Action::RepeatSearch(false),
            '\u{1b}' => Action::ClearSearch,
            _ => Action::Move(motion(key)?, count),
        })
    }
}
fn operator(c: char) -> Option<Operator> {
    Some(match c {
        'd' => Operator::Delete,
        'c' => Operator::Change,
        'y' => Operator::Yank,
        _ => return None,
    })
}
fn motion(c: char) -> Option<Motion> {
    Some(match c {
        'h' => Motion::Left,
        'l' => Motion::Right,
        'j' => Motion::Down,
        'k' => Motion::Up,
        'w' => Motion::Word,
        'W' => Motion::BigWord,
        'b' => Motion::BackWord,
        'B' => Motion::BigBackWord,
        'e' => Motion::WordEnd,
        'E' => Motion::BigWordEnd,
        '0' => Motion::Start,
        '^' => Motion::FirstNonBlank,
        '$' => Motion::End,
        '%' => Motion::MatchBracket,
        '{' => Motion::ParagraphBackward,
        '}' => Motion::ParagraphForward,
        'H' => Motion::ScreenTop,
        'M' => Motion::ScreenMiddle,
        'L' => Motion::ScreenBottom,
        'G' => Motion::LastLine,
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composed_counts_and_operators() {
        let mut p = CommandEngine::default();
        assert_eq!(p.feed('3'), None);
        assert_eq!(p.feed('d'), None);
        assert_eq!(p.feed('2'), None);
        assert_eq!(
            p.feed('w'),
            Some(Action::Operate(Operator::Delete, Motion::Word, 6))
        );
        assert_eq!(p.feed('g'), None);
        assert_eq!(p.feed('g'), Some(Action::Move(Motion::FirstLine, 1)));
        assert_eq!(p.feed('4'), None);
        assert_eq!(p.feed('y'), None);
        assert_eq!(p.feed('y'), Some(Action::Lines(Operator::Yank, 4)));
        assert_eq!(p.feed('g'), None);
        assert_eq!(p.feed('r'), Some(Action::References));
    }
    #[test]
    fn find_motion_compose_with_counts_and_operators() {
        let mut p = CommandEngine::default();
        assert_eq!(p.feed('2'), None);
        assert_eq!(p.feed('d'), None);
        assert_eq!(p.feed('f'), None);
        assert_eq!(
            p.feed('x'),
            Some(Action::Operate(
                Operator::Delete,
                Motion::Find {
                    target: 'x',
                    forward: true,
                    till: false,
                    repeat: false
                },
                2
            ))
        );
        assert_eq!(
            p.feed(';'),
            Some(Action::Move(
                Motion::Find {
                    target: 'x',
                    forward: true,
                    till: false,
                    repeat: true
                },
                1
            ))
        );
        assert_eq!(
            p.feed(','),
            Some(Action::Move(
                Motion::Find {
                    target: 'x',
                    forward: false,
                    till: false,
                    repeat: true
                },
                1
            ))
        );
        assert_eq!(p.feed('g'), None);
        assert_eq!(p.feed('f'), None);
        assert!(!p.has_pending());
    }
}
