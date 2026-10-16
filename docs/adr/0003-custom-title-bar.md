# 0003: 自绘标题栏（Windows/macOS），Linux 保留原生

## 状态

已接受（2026-10-16）

## 背景

Windows 原生标题栏与应用主题（配色、暗色模式、标签条样式）不一致。目标是完全自绘标题栏，并将现有标签条融合进标题栏一行。

## 决策

- 使用 gpui-component 的 `TitleBar` 组件：`main.rs` 通过 `TitleBar::title_bar_options()`（`appears_transparent: true`）启用透明系统标题栏；macOS 上 `render_tab_strip` 直接作为 `TitleBar` 的子元素渲染。
- Windows 上 `render_tab_strip` 自绘标题栏行：标签条居左，空白拖拽区通过 `.window_control_area(WindowControlArea::Drag)` 报告 `HTCAPTION`，三个窗口控制按钮通过 `.window_control_area(Min/Max/Close)` 走原生非客户区路径（gpui 的 `nc_button_pressed` 处理执行最小化/最大化-还原切换/关闭）；最大化图标按 `window.is_maximized()` 切换为还原图标。所有控件命中盒必须 `.occlude()`，否则根视图焦点转移监听器会对转发进来的 NC 鼠标按下调用 `prevent_default()`，使 gpui 误判点击已被应用处理，从而跳过全部原生标题栏行为。
- Linux 保留服务端装饰与独立标签条行，不启用透明标题栏。
- 窗口标题文字不在标题栏渲染；`set_window_title` 仅服务于任务栏/Alt-Tab。
- 不引入 Win32/DWM FFI，不 fork gpui。

## 理由

- gpui 0.2.2 的 Windows 后端已完整支持客户端标题栏（`WM_NCCALCSIZE` 边距、`WM_NCHITTEST` → `HTCAPTION`/`HTCLOSE` 等），`gpui-component` 0.5.1 自带 `TitleBar`，主题 token `title_bar`/`title_bar_border` 原本已在调色板中定义。
- Windows 上必须走 `window_control_area` 命中测试路径：事件驱动替代方案在 gpui 0.2.2 是死路——`start_window_move()` 仅 Wayland/X11 实现（Windows 为空操作），`zoom_window()` 只执行 `SW_MAXIMIZE` 永不还原。
- 不能直接用 `gpui_component::TitleBar`（其内部控件未 `occlude()`，会被本应用根视图的焦点转移监听器拦截 NC 点击），因此 Windows 使用自带 `occlude()` 的自绘行。
- 关闭按钮经 `WM_CLOSE` → `on_window_should_close` 走退出确认流程，与原生行为一致。
- Linux 例外：gpui 的 `window_decorations` 在 Linux 默认为 `Server`，启用客户端装饰需额外处理窗口菜单与阴影，收益低、风险高。

## 后果

- 同一窗口在 Linux 与其他平台外观不同（Linux 多一条原生标题栏）。
- Windows 上自绘最大化按钮无系统 snap 布局弹层（gpui 固有限制）。
- 未来新增 OS 窗口须沿用 `TitleBar::title_bar_options()` 模式。
