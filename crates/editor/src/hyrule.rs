//! The Hyrule colour scheme for the embedded Neovim.
//!
//! Hyrule is part of Rainglow (https://rainglow.io), by Dayle Rees, MIT
//! licensed: "Copyright 2017-18 Dayle Rees". The table below is generated
//! from its `colors/hyrule.vim`
//! (https://github.com/rainglow/vim/blob/master/colors/hyrule.vim); the
//! groups after it are derived from the same palette for the parts that
//! file leaves out (diagnostics, floating windows, git signs).

/// Sets the scheme, unless the user's own Neovim config already chose a
/// colour scheme. Returns whether it was applied.
pub const HYRULE_LUA: &str = r#"
if vim.g.colors_name ~= nil and vim.g.colors_name ~= 'default' then
  return false
end
local groups = {
  Cursor = { fg = '#2d2c2b', bg = '#f8f8f0' },
  Visual = { fg = '#ffffff', bg = '#569e16' },
  CursorLine = { bg = '#353432' },
  CursorLineNr = { fg = '#7b7876', bg = '#1b1a1a' },
  CursorColumn = { bg = '#1b1a1a' },
  ColorColumn = { bg = '#131212' },
  LineNr = { fg = '#474544', bg = '#201f1f' },
  VertSplit = { fg = '#474544', bg = '#474544' },
  MatchParen = { fg = '#569e16', underline = true },
  StatusLine = { fg = '#c0d5c1', bg = '#201f1f', bold = true },
  StatusLineNC = { fg = '#c0d5c1', bg = '#201f1f' },
  Pmenu = { fg = '#c0d5c1', bg = '#201f1f' },
  PmenuSel = { bg = '#f5c504' },
  IncSearch = { fg = '#c0d5c1', bg = '#f5c504' },
  Search = { underline = true },
  Directory = { fg = '#f5c504' },
  Folded = { fg = '#b1cbb2', bg = '#131212' },
  Normal = { fg = '#90c93f', bg = '#2d2c2b' },
  Boolean = { fg = '#90c93f' },
  Character = { fg = '#ce830d' },
  Comment = { fg = '#716d6a' },
  Conditional = { fg = '#90c93f' },
  Constant = {},
  Define = { fg = '#f5c504' },
  DiffAdd = { fg = '#545250', bg = '#a7da1e', bold = true },
  DiffDelete = { fg = '#545250', bg = '#e61f44' },
  DiffChange = { fg = '#545250', bg = '#f7b83d' },
  DiffText = { fg = '#545250', bg = '#f7b83d', bold = true },
  ErrorMsg = { fg = '#545250', bg = '#e61f44' },
  WarningMsg = { fg = '#545250', bg = '#f7b83d' },
  Float = { fg = '#f5c504' },
  Function = { fg = '#f5c504' },
  Identifier = { fg = '#c0d5c1' },
  Keyword = { fg = '#f5c504' },
  Label = { fg = '#ce830d' },
  NonText = { fg = '#507952', bg = '#282726' },
  Number = { fg = '#f5c504' },
  Operator = { fg = '#c0d5c1' },
  PreProc = { fg = '#979390' },
  Special = { fg = '#c0d5c1' },
  SpecialKey = { fg = '#c0d5c1', bg = '#f5c504' },
  Statement = { fg = '#90c93f' },
  StorageClass = { fg = '#569e16' },
  String = { fg = '#ce830d' },
  Tag = { fg = '#f5c504' },
  Title = { fg = '#f5c504', bold = true },
  Todo = { fg = '#979390', reverse = true, bold = true },
  Type = {},
  Underlined = { underline = true },
  phpIdentifier = { fg = '#c0d5c1' },
  phpMethodsVar = { fg = '#dfdede' },
  xmlTag = { fg = '#f5c504' },
  xmlTagName = { fg = '#f5c504' },
  xmlEndTag = { fg = '#f5c504' },
}
-- Derived from the palette: what hyrule.vim does not cover.
local derived = {
  NormalFloat = { fg = '#c0d5c1', bg = '#201f1f' },
  FloatBorder = { fg = '#507952', bg = '#201f1f' },
  PmenuSbar = { bg = '#353432' },
  PmenuThumb = { bg = '#716d6a' },
  EndOfBuffer = { fg = '#2d2c2b' },
  SignColumn = { bg = '#201f1f' },
  WinSeparator = { fg = '#474544' },
  DiagnosticError = { fg = '#e61f44' },
  DiagnosticWarn = { fg = '#f7b83d' },
  DiagnosticInfo = { fg = '#569e16' },
  DiagnosticHint = { fg = '#979390' },
  DiagnosticUnderlineError = { sp = '#e61f44', underline = true },
  DiagnosticUnderlineWarn = { sp = '#f7b83d', underline = true },
  DiagnosticUnderlineInfo = { sp = '#569e16', underline = true },
  DiagnosticUnderlineHint = { sp = '#979390', underline = true },
  LfMideAdd = { fg = '#a7da1e' },
  LfMideChange = { fg = '#f7b83d' },
  LfMideDelete = { fg = '#e61f44' },
}
vim.o.background = 'dark'
vim.g.colors_name = 'hyrule'
for name, spec in pairs(groups) do
  vim.api.nvim_set_hl(0, name, spec)
end
for name, spec in pairs(derived) do
  vim.api.nvim_set_hl(0, name, spec)
end
return true
"#;
