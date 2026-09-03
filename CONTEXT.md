# OxideSSH

OxideSSH 是一个跨平台 SSH 客户端：管理保存的连接配置，并在标签页中运行交互式终端会话。本文件是整个仓库的统一术语表。

## Language

### 连接与凭据

**Profile**:
一条保存的 SSH 连接配置：名称、Endpoint、用户名和 AuthConfig。对应代码中的 `ConnectionProfile`。
_Avoid_: Connection、Server、Host 配置（UI 文案里的 "Hosts" 指 Profile 列表，不是术语）

**Endpoint**:
一对 `host:port` 地址，标识一台可达的 SSH 服务器。不含用户名与认证信息。

**Credential**:
存储在操作系统原生凭据库（Credential Manager / Keychain / Secret Service）中的秘密（密码或私钥口令）。明文从不序列化到磁盘，也从不写入日志。

**CredentialRef**:
指向 Credential 的引用标识，持久化在 Profile 中代替秘密本身。Profile 文件里只允许出现 CredentialRef。

### 会话与标签页

**Session**:
一条活跃的 SSH 通道及其运行时状态机（连接中、等待主机密钥、已连接、已断开）。对应 `SessionHandle` / `SessionState`。

**Tab**:
一个 Session 与其 TerminalModel 的编排单元，是 UI 标签页背后的领域对象。Tab 不依赖任何 UI 工具包。
_Avoid_: Session 标签、Terminal tab

**ModalRequest**:
Tab 向 UI 发起的一次模态请求（主机密钥确认、秘密输入、关闭确认），经 `ModalQueue` 按 FIFO 递送。陈旧回调不得消费他人请求的提示。

**Workspace**:
管理 Profile 的主界面（增删改、搜索、发起连接）。在标签栏中固定为索引 0 的常驻标签。
_Avoid_: 主页、Hosts 页

### 存储

**KnownHost**:
一条已信任的主机公钥记录（指纹与接受时间），UI 中称为 "Trusted Hosts"。

**Recovery**:
存储根目录损坏时的降级启动模式：应用以只读错误界面启动，绝不覆写损坏的配置文件。
