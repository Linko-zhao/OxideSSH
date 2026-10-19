# 设置项路线图（候选清单）

> 生成日期：2026-10-19。基于对 `AppSettings`（仅 `locale` + `theme`）与全仓硬编码参数的调查。
> 排序口径：按痛点频率分 P0/P1/P2，同级内按 价值×实现成本 排序。

## ⚠️ Schema 约束（实现任何一项前必读）

- `config.json` 的 `schemaVersion` 必须恒为 1，**不能 bump**（否则旧文件被判 `UnsupportedSchema`，进 Recovery 只读界面）。
- 所有新增字段必须加 `#[serde(default)]`（`AppSettings` 现有的 `locale`/`theme` 都没加，借机一并补上）。
- 新增项固定落点四处：`oxide-ssh-core/src/model.rs`（字段）→ `oxide-ssh-desktop/src/state/mod.rs`（setter）→ `app/views/settings.rs`（UI 控件）→ `i18n/{mod,en,zh_cn}.rs`（新增 `MessageId` 并加入 `MessageId::ALL`）。
- Profile 级项统一建模为：全局 `AppSettings` 存默认值 + `ConnectionProfile` 加 `Option<T>` 覆盖字段。

---

## 纯设置项

### P0 — 每天都会碰到的硬编码缺失

| # | 键名 | 现状 / 价值 | 成本 | 特殊落点 |
|---|------|-------------|------|----------|
| 1 | `terminal.fontSize` | 硬编码 `px(13.)`（`views/mod.rs:84`），高分屏/投影演示刚需 | 中 | 必须同步 `estimated_terminal_size`（`app/mod.rs:457-470`）的宽度系数 0.6，否则初始 PTY 尺寸错误 |
| 2 | `terminal.fontFamily` | 编译期常量：Win `Cascadia Mono` / macOS `Menlo`（`app/terminal.rs:4-11`） | 中 | 同上，须同步尺寸估算 |
| 3 | `terminal.lineHeightFactor` | 行高 `px(17.)` 硬编码；放开字号后必须随之可调 | 低 | 同 #1 |
| 4 | `ssh.connectTimeoutSeconds` | 15s 硬编码（`session.rs:36`），弱网/高延迟主机频繁超时 | 低 | `session.rs` |
| 5 | `ssh.keepaliveIntervalSeconds` / `ssh.keepaliveMaxCount` | 30s×3 硬编码（`session.rs:647-652`），NAT/防火墙掉线痛点；profile 可覆盖 | 低 | `session.rs` + profile `Option` |
| 6 | `local.shellPath` / `local.shellArgs` | Windows 只能吃 `pwsh→powershell→cmd` 回退链（`local.rs`），无法指定 git-bash/wsl | 低 | `local.rs::default_shell` |
| 7 | `terminal.scrollbackLines` | 固定 10,000 行（`oxide-ssh-terminal/src/model.rs:20`），长日志回滚不够 | 低 | alacritty `Scrolling` 配置 |

### P1 — 常见需求

| # | 键名 | 现状 / 价值 | 成本 | 特殊落点 |
|---|------|-------------|------|----------|
| 1 | `terminal.cursorStyle` / `terminal.cursorBlink` | 光标跟随远端应用但 `CursorBlinkingChange` 事件被忽略（`app/terminal.rs:245-270`） | 中 | terminal 事件分支 |
| 2 | `terminal.bell`（`visual`/`sound`/`off`） | 现仅视觉 Alert，无声、无开关（`app/mod.rs:697-702`） | 低 | — |
| 3 | `terminal.osc52Clipboard` | OSC 52 远端写剪贴板已禁用；安全敏感，需显式三态（off/ask/on） | 低 | terminal model 事件 + 确认弹窗 |
| 4 | `terminal.copyOnSelect` | 选中即复制，Unix 用户习惯 | 中 | 终端选区/鼠标路径 |
| 5 | `terminal.pasteConfirmThreshold` | 粘贴上限 1 MiB 硬编码；大段粘贴（尤其多行命令）应有确认防注入 | 低 | 粘贴路径 + i18n 文案 |
| 6 | `ui.confirmBeforeCloseWithSessions` | 有活动会话时关闭标签/窗口需确认，防误关丢工作 | 中 | 标签/窗口关闭路径 |
| 7 | `profile.startupCommand` | 连接成功后自动执行（如 `tmux attach`），profile 字段 | 中 | session 建立后写 channel |
| 8 | `local.workingDirectory` | 本地终端 cwd 固定为 home | 低 | `local.rs` |
| 9 | `window.rememberSize` / 启动时恢复上次窗口尺寸 | 每次启动重新调整 | 中 | `main.rs` window options + 存 `config.json` |

### P2 — 小众/锦上添花

| # | 键名 | 现状 / 价值 | 成本 | 特殊落点 |
|---|------|-------------|------|----------|
| 1 | `ssh.term` | TERM 固定 `xterm-256color`（`session.rs:703-705`），仅 tmux/特殊场景需要 | 低 | `session.rs` |
| 2 | `ssh.compression` | 慢链路有用；需确认 russh 0.62 压缩支持现状 | 中 | `session.rs` |
| 3 | `profile.environmentVariables` | 服务端 `AcceptEnv` 默认很少开，受众小 | 低 | `session.rs` + profile |
| 4 | `terminal.ansiPalette`（16 色自定义） | 现 16 色固定且不随主题变（`theme.rs:30-56`）；需配色编辑 UI | 高 | `theme.rs` |
| 5 | `ui.uiScale` | 依赖 GPUI 缩放能力，需先验证 | 中 | 全局渲染 |
| 6 | `security.clearAllCredentials` | 一键清空 keyring 中本应用凭据；需可枚举 `profile/{uuid}/*` 引用 | 中 | `credentials.rs` |
| 7 | `knownHosts` 导入/导出 | 换机迁移 `known_hosts.json` | 中 | `storage.rs` + 文件对话框 |
| 8 | profile 级外观覆盖（每连接独立配色/字号） | 用颜色区分生产/测试机 | 中 | `theme.rs` + 终端视图 |

---

## 功能级项（披着设置外衣，成本远大于普通设置）

| # | 功能 | 价值 | 成本 | 说明 |
|---|------|------|------|------|
| 1 | ProxyJump / 跳板机 | 高（运维场景刚需） | 高 | russh 多跳 channel 嵌套 + profile 增加 `jumpHost` 字段 |
| 2 | 端口转发（local/remote/dynamic SOCKS） | 高 | 高 | TCP 转发监听器 + 转发规则管理 UI |
| 3 | Agent forwarding 开关 | 中 | 中高 | 需实现 agent channel 转发；做成 profile 布尔项 |
| 4 | 快捷键自定义 | 中 | 高 | 需 keymap 配置层 + 冲突检测 UI |
| 5 | 配置导入/导出/备份 | 中 | 中 | `config.json` 易；keyring 凭据不可导出，导出文件需声明此限制 |

---

## 建议实施顺序

1. **第一批（一次 PR）**：P0 #4–#7 + P1 #2 #5 #8 — 全是"硬编码常量 → 配置字段"的机械改动，成本全低，顺带补 `#[serde(default)]`。
2. **第二批**：P0 #1–#3（字体三件套，需一起解决尺寸估算联动）+ P1 #6。
3. **第三批**：P1 其余 + 按反馈挑 P2。
4. 功能级项各自单独立项，不与设置项混排工期。
