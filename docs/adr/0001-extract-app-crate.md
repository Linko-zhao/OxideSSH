# 新建 oxide-ssh-app crate 承载 UI 无关的编排逻辑

**Status**: accepted

`Tab` / `TabCollection` / `ModalQueue` 是零 GPUI 依赖的领域状态机（约 1100 行，自带测试），此前住在 UI crate `oxide-ssh-desktop` 里，模糊了"界面工程"的边界。我们将它们下沉为独立的 `oxide-ssh-app` crate，依赖 `oxide-ssh-core` 与 `oxide-ssh-terminal`，desktop 只保留渲染与交互。

## Considered Options

- **并入 oxide-ssh-core 并给它加 terminal 依赖**：被拒绝。core 的定位是"可移植配置、存储、信任与 SSH 传输"，依赖终端模拟器会稀释这一定位，且让 core 的 API 面不必要地变大。
- **留在 desktop**：被拒绝。UI crate 会继续堆积领域逻辑（ConnectionForm、凭据编排已出现同样趋势），且该状态机无法脱离 GPUI 独立测试。

## Consequences

- workspace 从 3 个 crate 变为 4 个。
- 未来从 desktop 解耦出的应用层逻辑（如 `ProfileCredentialCoordinator`、`ConnectionForm`）应归入 `oxide-ssh-app`，而不是 core 或 desktop。
- `ModalRequest` 的谓词方法与 `ModalQueue::complete_current_if` 因此从 `pub(crate)` 放宽为 `pub`，成为该 crate 的公开 API。
