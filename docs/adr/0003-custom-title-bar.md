# 0003: 自绘标题栏（Windows/macOS），Linux 保留原生

## 状态

已接受（2026-10-16）

## 背景

Windows 原生标题栏与应用主题（配色、暗色模式、标签条样式）不一致。目标是完全自绘标题栏，并将现有标签条融合进标题栏一行。

## 决策

- 使用 gpui-component 的 `TitleBar` 组件：`main.rs` 通过 `TitleBar::title_bar_options()`（`appears_transparent: true`）启用透明系统标题栏，`render_tab_strip` 在 Windows/macOS 上作为 `TitleBar` 的子元素渲染，品牌名居左、标签条居中、窗口控制按钮（最小化/最大化/关闭）由 `TitleBar` 居右自绘。
- Linux 保留服务端装饰与独立标签条行，不启用透明标题栏。
- 窗口标题文字不在标题栏渲染；`set_window_title` 仅服务于任务栏/Alt-Tab。
- 不引入 Win32/DWM FFI，不 fork gpui。

## 理由

- gpui 0.2.2 的 Windows 后端已完整支持客户端标题栏（`WM_NCCALCSIZE` 边距、`WM_NCHITTEST` → `HTCAPTION`/`HTCLOSE` 等），`gpui-component` 0.5.1 自带 `TitleBar`，主题 token `title_bar`/`title_bar_border` 原本已在调色板中定义。
- Linux 例外：gpui 的 `window_decorations` 在 Linux 默认为 `Server`，启用客户端装饰需额外处理窗口菜单与阴影，收益低、风险高。

## 后果

- 同一窗口在 Linux 与其他平台外观不同（Linux 多一条原生标题栏）。
- Windows 上自绘最大化按钮无系统 snap 布局弹层（gpui 固有限制）。
- 未来新增 OS 窗口须沿用 `TitleBar::title_bar_options()` 模式。
