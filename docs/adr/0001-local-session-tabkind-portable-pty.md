# 本地会话：TabKind 枚举 + portable-pty

OxideSSH 增加了「本机终端」入口：在应用内嵌终端标签页中运行平台默认 shell。两个决策：

1. **Tab 用 `TabKind { Ssh(ConnectionProfile), Local }` 枚举区分会话目标**，而不是为本地会话伪造一个不持久化的合成 profile。假 profile 会把无意义的 endpoint/认证数据带进密钥查找、主机密钥确认、标签过滤等 SSH 专属逻辑；枚举让 `profile()` 返回 `Option`，编译器强制每个调用点处理本地情形。

2. **本地 PTY 后端引入 `portable-pty` 依赖**，而非手写 forkpty/ConPTY。一个成熟依赖换跨平台（含 Windows ConPTY）完整实现；本地会话复用与 SSH 相同的 `SessionEvent` 协议（Output/Exited/StateChanged），因此 UI 事件泵、断连 overlay、Retry（语义为重启 shell）全部零改动复用。

本地会话绕过整个密钥/主机密钥管线：不需要 `ProfileId` 键控的 connecting/secret 集合。
