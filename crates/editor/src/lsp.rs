//! Language servers for the embedded Neovim, set up with Neovim's own LSP
//! client (`vim.lsp.config` / `vim.lsp.enable`, Neovim 0.11+) and its
//! built-in completion, so no plugin is needed.

use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use nvim_rs::Value;

/// Registers a server per language that has one and turns on completion as
/// you type for those servers. Paths arrive as data (arguments), never as
/// part of the code.
pub const SETUP_LUA: &str = r#"
local servers = ...
local function add(name, cmd, filetypes, markers)
  vim.lsp.config(name, { cmd = { cmd }, filetypes = filetypes, root_markers = markers })
  vim.lsp.enable(name)
end
if servers.kotlin then
  add('lf_mide_kotlin', servers.kotlin, { 'kotlin', 'java' },
    { 'settings.gradle.kts', 'settings.gradle', '.git' })
end
if servers.swift then
  add('lf_mide_swift', servers.swift, { 'swift', 'objc', 'objcpp' },
    { 'buildServer.json', 'Package.swift', '.git' })
end
if not servers.kotlin and not servers.swift then return 0 end
-- Completion pops up while typing; nothing is inserted until one is chosen.
vim.opt.completeopt = { 'menu', 'menuone', 'noselect', 'popup' }
vim.api.nvim_create_autocmd('LspAttach', {
  group = vim.api.nvim_create_augroup('lf_mide_lsp', { clear = true }),
  callback = function(event)
    local client = vim.lsp.get_client_by_id(event.data.client_id)
    if client and vim.startswith(client.name, 'lf_mide_')
        and client:supports_method('textDocument/completion') then
      vim.lsp.completion.enable(true, client.id, event.buf, { autotrigger = true })
    end
  end,
})
return (servers.kotlin and 1 or 0) + (servers.swift and 1 or 0)
"#;

/// The servers to use, by language. `None` when that server is not installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LspServers {
    pub kotlin: Option<PathBuf>,
    pub swift: Option<PathBuf>,
}

impl LspServers {
    /// The argument [`SETUP_LUA`] expects: a table with a path per language.
    pub fn setup_args(&self) -> Vec<Value> {
        let entry = |name: &str, path: &Option<PathBuf>| {
            path.as_ref().map(|path| {
                (
                    Value::from(name),
                    Value::Binary(path.as_os_str().as_bytes().to_vec()),
                )
            })
        };
        let table = [entry("kotlin", &self.kotlin), entry("swift", &self.swift)]
            .into_iter()
            .flatten()
            .collect();
        vec![Value::Map(table)]
    }

    pub fn is_empty(&self) -> bool {
        self.kotlin.is_none() && self.swift.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_installed_servers_are_passed() {
        let servers = LspServers {
            kotlin: Some(PathBuf::from("/bin/kmp-lsp")),
            swift: None,
        };

        let args = servers.setup_args();

        assert_eq!(
            args,
            vec![Value::Map(vec![(
                Value::from("kotlin"),
                Value::Binary(b"/bin/kmp-lsp".to_vec())
            )])]
        );
        assert!(!servers.is_empty());
        assert!(LspServers::default().is_empty());
    }
}
