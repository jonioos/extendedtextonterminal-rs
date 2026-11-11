use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub ui: Ui,
    pub search: SearchOptions,
    pub editing: EditingOptions,
    pub keys: Keys,
    pub languages: HashMap<String, LanguageServer>,
    pub tools: HashMap<String, LanguageTools>,
}
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ToolCommand {
    pub command: Vec<String>,
    pub stdin: bool,
}
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct LanguageTools {
    pub formatter: Option<ToolCommand>,
    pub linter: Option<ToolCommand>,
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EditingOptions {
    pub autoindent: bool,
    pub shiftwidth: usize,
}
impl Default for EditingOptions {
    fn default() -> Self {
        Self {
            autoindent: false,
            shiftwidth: 4,
        }
    }
}
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct SearchOptions {
    pub ignorecase: bool,
    pub smartcase: bool,
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub line_numbers: bool,
    pub relative_numbers: bool,
    pub theme: String,
}
impl Default for Ui {
    fn default() -> Self {
        Self {
            line_numbers: true,
            relative_numbers: false,
            theme: "dark".into(),
        }
    }
}
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Keys {
    pub normal: HashMap<String, String>,
}
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct LanguageServer {
    pub command: Vec<String>,
    pub language_id: String,
}
impl Config {
    pub fn formatter_for(&self, extension: &str) -> Option<ToolCommand> {
        if let Some(custom) = self
            .tools
            .get(extension)
            .and_then(|tools| tools.formatter.as_ref())
        {
            return (!custom.command.is_empty()).then(|| custom.clone());
        }
        let command: &[&str] = match extension {
            "rs" => &["rustfmt", "--emit", "stdout"],
            "py" => &["ruff", "format", "-"],
            "go" => &["gofmt"],
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hxx" => &["clang-format"],
            "js" | "jsx" | "ts" | "tsx" | "html" | "htm" | "css" => {
                &["prettier", "--stdin-filepath", "{file}"]
            }
            _ => return None,
        };
        Some(ToolCommand {
            command: command.iter().map(|part| (*part).into()).collect(),
            stdin: true,
        })
    }
    pub fn linter_for(&self, extension: &str) -> Option<ToolCommand> {
        if let Some(custom) = self
            .tools
            .get(extension)
            .and_then(|tools| tools.linter.as_ref())
        {
            return (!custom.command.is_empty()).then(|| custom.clone());
        }
        let (command, stdin): (&[&str], bool) = match extension {
            "rs" => (&["cargo", "check", "--message-format=short"], false),
            "py" => (
                &[
                    "ruff",
                    "check",
                    "--output-format",
                    "concise",
                    "--stdin-filename",
                    "{file}",
                    "-",
                ],
                true,
            ),
            "go" => (&["go", "vet", "{file}"], false),
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hxx" => {
                (&["clang-tidy", "{file}", "--quiet"], false)
            }
            "js" | "jsx" | "ts" | "tsx" => {
                (&["eslint", "--stdin", "--stdin-filename", "{file}"], true)
            }
            _ => return None,
        };
        Some(ToolCommand {
            command: command.iter().map(|part| (*part).into()).collect(),
            stdin,
        })
    }
    pub fn apply_set(&mut self, command: &str) -> Result<String, String> {
        let option = command.trim();
        match option {
            "number" | "nu" => self.ui.line_numbers = true,
            "nonumber" | "nonu" => self.ui.line_numbers = false,
            "relativenumber" | "rnu" => self.ui.relative_numbers = true,
            "norelativenumber" | "nornu" => self.ui.relative_numbers = false,
            "background=dark" => self.ui.theme = "dark".into(),
            "background=light" => self.ui.theme = "light".into(),
            "ignorecase" | "ic" => self.search.ignorecase = true,
            "noignorecase" | "noic" => self.search.ignorecase = false,
            "smartcase" | "scs" => self.search.smartcase = true,
            "nosmartcase" | "noscs" => self.search.smartcase = false,
            "autoindent" | "ai" => self.editing.autoindent = true,
            "noautoindent" | "noai" => self.editing.autoindent = false,
            "" => {
                return Ok(format!(
                    "number={} relativenumber={} background={}",
                    self.ui.line_numbers, self.ui.relative_numbers, self.ui.theme
                ))
            }
            "number?" | "nu?" => return Ok(format!("number={}", self.ui.line_numbers)),
            "relativenumber?" | "rnu?" => {
                return Ok(format!("relativenumber={}", self.ui.relative_numbers))
            }
            "background?" => return Ok(format!("background={}", self.ui.theme)),
            "ignorecase?" | "ic?" => return Ok(format!("ignorecase={}", self.search.ignorecase)),
            "smartcase?" | "scs?" => return Ok(format!("smartcase={}", self.search.smartcase)),
            "autoindent?" | "ai?" => return Ok(format!("autoindent={}", self.editing.autoindent)),
            "shiftwidth?" | "sw?" => return Ok(format!("shiftwidth={}", self.editing.shiftwidth)),
            _ if option.starts_with("shiftwidth=") || option.starts_with("sw=") => {
                let width = option
                    .split_once('=')
                    .unwrap()
                    .1
                    .parse::<usize>()
                    .map_err(|_| "Invalid shiftwidth".to_string())?;
                if !(1..=32).contains(&width) {
                    return Err("shiftwidth must be 1..32".into());
                }
                self.editing.shiftwidth = width;
            }
            _ => return Err(format!("Unknown option: {option}")),
        }
        Ok(format!("set {option}"))
    }
    pub fn server_for(&self, extension: &str) -> Option<LanguageServer> {
        if let Some(server) = self.languages.get(extension) {
            return (!server.command.is_empty()).then(|| server.clone());
        }
        let (command, language_id): (&[&str], &str) = match extension {
            "rs" => (&["rust-analyzer"], "rust"),
            "ts" | "tsx" => (&["typescript-language-server", "--stdio"], "typescript"),
            "js" | "jsx" => (&["typescript-language-server", "--stdio"], "javascript"),
            "py" => (&["pyright-langserver", "--stdio"], "python"),
            "go" => (&["gopls"], "go"),
            "c" | "h" => (&["clangd"], "c"),
            "cc" | "cpp" | "cxx" | "hpp" | "hxx" => (&["clangd"], "cpp"),
            "html" | "htm" => (&["vscode-html-language-server", "--stdio"], "html"),
            "css" => (&["vscode-css-language-server", "--stdio"], "css"),
            "cs" => (&["csharp-ls"], "csharp"),
            "vb" => (&["vbnet-ls"], "vb"),
            _ => return None,
        };
        Some(LanguageServer {
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            language_id: language_id.into(),
        })
    }
}
impl Config {
    pub fn load() -> io::Result<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        let path = base.join("etot/config.toml");
        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let config: Self = toml::from_str(&data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        })?;
        if config.ui.theme != "dark" && config.ui.theme != "light" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "theme must be 'dark' or 'light'",
            ));
        }
        if !(1..=32).contains(&config.editing.shiftwidth) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shiftwidth must be 1..32",
            ));
        }
        for (from, to) in &config.keys.normal {
            if from.chars().count() != 1 || to.is_empty() || to.chars().count() > 64 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "normal key maps must map one character to 1–64 characters",
                ));
            }
        }
        Ok(config)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn set_options_update_runtime_config() {
        let mut config = Config::default();
        config.apply_set("nonumber").unwrap();
        config.apply_set("relativenumber").unwrap();
        config.apply_set("background=light").unwrap();
        config.apply_set("ignorecase").unwrap();
        config.apply_set("smartcase").unwrap();
        assert!(!config.ui.line_numbers);
        assert!(config.ui.relative_numbers);
        assert_eq!(config.ui.theme, "light");
        assert!(config.search.ignorecase && config.search.smartcase);
        assert_eq!(config.apply_set("number?").unwrap(), "number=false");
        assert!(config.apply_set("unknown").is_err());
    }
    #[test]
    fn built_in_language_servers_cover_requested_languages() {
        let config = Config::default();
        for (extension, language) in [
            ("py", "python"),
            ("go", "go"),
            ("cpp", "cpp"),
            ("html", "html"),
            ("cs", "csharp"),
            ("vb", "vb"),
        ] {
            let server = config.server_for(extension).unwrap();
            assert_eq!(server.language_id, language);
            assert!(!server.command.is_empty());
        }
    }
}
