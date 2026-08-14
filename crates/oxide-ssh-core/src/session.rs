use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_channel::{Receiver, Sender, TrySendError};
use bytes::Bytes;
use parking_lot::Mutex;
use russh::{
    ChannelMsg, Disconnect, client,
    keys::{
        PrivateKeyWithHashAlg,
        agent::client::{AgentClient, AgentStream},
        ssh_key::{self, HashAlg},
    },
};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;
use tokio::{runtime::Runtime, sync::watch, task::AbortHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    model::{AuthConfig, ConnectionProfile, Endpoint, SessionId, TerminalSize},
    storage::{AppStore, KnownHostEntry, StorageError, canonicalize_endpoint},
};

const INPUT_QUEUE_CAPACITY: usize = 128;
const EVENT_QUEUE_CAPACITY: usize = 256;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HOST_KEY_DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const DISCONNECT_GRACE: Duration = Duration::from_secs(2);

pub struct ConnectRequest {
    pub profile: ConnectionProfile,
    pub secret: Option<SecretString>,
    pub initial_size: TerminalSize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostKeyDecision {
    AcceptAndStore,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Connecting,
    VerifyingHostKey,
    Authenticating,
    OpeningShell,
    Connected,
    Disconnected,
}

#[derive(Debug)]
pub enum SessionEvent {
    StateChanged(SessionState),
    UnknownHostKey {
        prompt_id: Uuid,
        endpoint: Endpoint,
        algorithm: String,
        fingerprint_sha256: String,
    },
    ChangedHostKey {
        endpoint: Endpoint,
        expected_sha256: String,
        presented_sha256: String,
    },
    Output(Bytes),
    Bell,
    Exited {
        status: Option<u32>,
    },
    Error(SessionError),
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SendInputError {
    #[error("session input queue is full")]
    QueueFull,
    #[error("session input queue is closed")]
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SessionError {
    #[error("invalid connection profile")]
    InvalidProfile,
    #[error("connection timed out")]
    ConnectTimeout,
    #[error("connection failed")]
    ConnectFailed,
    #[error("host key rejected")]
    HostKeyRejected,
    #[error("host key changed")]
    HostKeyChanged,
    #[error("host key store failed")]
    HostKeyStoreFailed,
    #[error("credential unavailable")]
    CredentialUnavailable,
    #[error("private key unreadable")]
    PrivateKeyUnreadable,
    #[error("private key passphrase required")]
    PrivateKeyPassphraseRequired,
    #[error("private key passphrase rejected")]
    PrivateKeyPassphraseRejected,
    #[error("SSH agent unavailable")]
    AgentUnavailable,
    #[error("SSH agent has no identities")]
    AgentEmpty,
    #[error("authentication rejected")]
    AuthenticationRejected,
    #[error("PTY request rejected")]
    PtyRejected,
    #[error("shell request rejected")]
    ShellRejected,
    #[error("session disconnected")]
    Disconnected,
}

impl From<russh::Error> for SessionError {
    fn from(_: russh::Error) -> Self {
        Self::ConnectFailed
    }
}

pub struct SshService {
    runtime: Arc<Runtime>,
    trust: Arc<HostTrustCoordinator>,
    agent_connector: Arc<dyn AgentConnector>,
}

impl SshService {
    pub fn new(store: Arc<AppStore>) -> Result<Self, SessionError> {
        Self::with_agent_connector(store, Arc::new(SystemAgentConnector))
    }

    fn with_agent_connector(
        store: Arc<AppStore>,
        agent_connector: Arc<dyn AgentConnector>,
    ) -> Result<Self, SessionError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("oxide-ssh-net")
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| SessionError::ConnectFailed)?;
        Ok(Self {
            runtime: Arc::new(runtime),
            trust: Arc::new(HostTrustCoordinator::new(store)),
            agent_connector,
        })
    }

    pub fn connect(&self, request: ConnectRequest) -> Result<SessionHandle, SessionError> {
        request
            .profile
            .validate()
            .map_err(|_| SessionError::InvalidProfile)?;
        if !request.initial_size.is_valid() {
            return Err(SessionError::InvalidProfile);
        }

        let id = SessionId::new();
        let (event_sender, event_receiver) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
        let (input_sender, input_receiver) = async_channel::bounded(INPUT_QUEUE_CAPACITY);
        let (resize_sender, resize_receiver) = watch::channel(request.initial_size);
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let trust = self.trust.clone();
        let agent_connector = self.agent_connector.clone();
        let task = self.runtime.spawn(async move {
            let _ = event_sender
                .send(SessionEvent::StateChanged(SessionState::Connecting))
                .await;
            let result = run_session(
                request,
                input_receiver,
                resize_receiver,
                event_sender.clone(),
                task_cancellation,
                trust,
                agent_connector,
            )
            .await;
            match result {
                Ok(status) => {
                    let _ = event_sender.send(SessionEvent::Exited { status }).await;
                }
                Err(SessionError::Disconnected) => {}
                Err(error) => {
                    let _ = event_sender.send(SessionEvent::Error(error)).await;
                }
            }
            let _ = event_sender
                .send(SessionEvent::StateChanged(SessionState::Disconnected))
                .await;
        });

        Ok(SessionHandle {
            id,
            events: Some(event_receiver),
            input: input_sender,
            resize: resize_sender,
            cancellation,
            task_abort: task.abort_handle(),
            runtime: self.runtime.clone(),
            trust: self.trust.clone(),
            disconnect_started: AtomicBool::new(false),
        })
    }
}

pub struct SessionHandle {
    id: SessionId,
    events: Option<Receiver<SessionEvent>>,
    input: Sender<Bytes>,
    resize: watch::Sender<TerminalSize>,
    cancellation: CancellationToken,
    task_abort: AbortHandle,
    runtime: Arc<Runtime>,
    trust: Arc<HostTrustCoordinator>,
    disconnect_started: AtomicBool,
}

impl SessionHandle {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn take_events(&mut self) -> Option<Receiver<SessionEvent>> {
        self.events.take()
    }

    pub fn try_send_input(&self, bytes: Bytes) -> Result<(), SendInputError> {
        self.input.try_send(bytes).map_err(|error| match error {
            TrySendError::Full(_) => SendInputError::QueueFull,
            TrySendError::Closed(_) => SendInputError::Closed,
        })
    }

    pub fn resize(&self, size: TerminalSize) {
        if size.is_valid() {
            self.resize.send_replace(size);
        }
    }

    pub fn decide_host_key(
        &self,
        prompt_id: Uuid,
        decision: HostKeyDecision,
    ) -> Result<(), SessionError> {
        self.trust.decide(prompt_id, decision)
    }

    pub fn disconnect(&self) {
        if self.disconnect_started.swap(true, Ordering::AcqRel) {
            return;
        }
        self.input.close();
        self.cancellation.cancel();
        let abort = self.task_abort.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(DISCONNECT_GRACE).await;
            if !abort.is_finished() {
                abort.abort();
            }
        });
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[derive(Clone)]
struct PresentedHostKey {
    algorithm: String,
    public_key: String,
    fingerprint_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrustOutcome {
    Waiting,
    Accepted,
    Rejected,
    Conflict,
}

struct PendingTrust {
    prompt_id: Uuid,
    presented: PresentedHostKey,
    outcome: watch::Sender<TrustOutcome>,
    event_senders: Vec<(Uuid, Sender<SessionEvent>)>,
    deciding: bool,
}

struct HostTrustCoordinator {
    store: Arc<AppStore>,
    pending: Mutex<HashMap<Endpoint, PendingTrust>>,
    awaiting: Mutex<HashMap<Endpoint, watch::Sender<bool>>>,
}

impl HostTrustCoordinator {
    fn new(store: Arc<AppStore>) -> Self {
        Self {
            store,
            pending: Mutex::new(HashMap::new()),
            awaiting: Mutex::new(HashMap::new()),
        }
    }

    fn set_awaiting(&self, endpoint: &Endpoint, value: bool) {
        if let Some(sender) = self.awaiting.lock().get(endpoint) {
            let _ = sender.send_replace(value);
        }
    }

    fn subscribe_awaiting(&self, endpoint: &Endpoint) -> watch::Receiver<bool> {
        self.awaiting
            .lock()
            .entry(endpoint.clone())
            .or_insert_with(|| watch::channel(false).0)
            .subscribe()
    }

    async fn verify(
        &self,
        endpoint: &Endpoint,
        presented: PresentedHostKey,
        events: Sender<SessionEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), SessionError> {
        let endpoint = canonicalize_endpoint(endpoint).map_err(|_| SessionError::InvalidProfile)?;
        match self.store.known_host(&endpoint) {
            Ok(Some(known)) => {
                if known.algorithm == presented.algorithm
                    && known.public_key == presented.public_key
                    && known.fingerprint_sha256 == presented.fingerprint_sha256
                {
                    return Ok(());
                }
                let _ = events
                    .send(SessionEvent::ChangedHostKey {
                        endpoint,
                        expected_sha256: known.fingerprint_sha256,
                        presented_sha256: presented.fingerprint_sha256,
                    })
                    .await;
                return Err(SessionError::HostKeyChanged);
            }
            Ok(None) => {}
            Err(_) => return Err(SessionError::HostKeyStoreFailed),
        }

        let (prompt_id, mut outcome, is_owner, waiter_token) = {
            let mut pending = self.pending.lock();
            if let Some(existing) = pending.get_mut(&endpoint) {
                if same_presented_key(&existing.presented, &presented) {
                    let waiter_token = Uuid::new_v4();
                    existing.event_senders.push((waiter_token, events.clone()));
                    (
                        existing.prompt_id,
                        existing.outcome.subscribe(),
                        false,
                        waiter_token,
                    )
                } else {
                    let existing = pending
                        .remove(&endpoint)
                        .expect("pending entry disappeared");
                    self.set_awaiting(&endpoint, false);
                    let expected = existing.presented.fingerprint_sha256;
                    existing.outcome.send_replace(TrustOutcome::Conflict);
                    for (_, sender) in existing
                        .event_senders
                        .into_iter()
                        .chain(std::iter::once((Uuid::new_v4(), events.clone())))
                    {
                        let _ = sender.try_send(SessionEvent::ChangedHostKey {
                            endpoint: endpoint.clone(),
                            expected_sha256: expected.clone(),
                            presented_sha256: presented.fingerprint_sha256.clone(),
                        });
                    }
                    return Err(SessionError::HostKeyChanged);
                }
            } else {
                let prompt_id = Uuid::new_v4();
                let (sender, receiver) = watch::channel(TrustOutcome::Waiting);
                pending.insert(
                    endpoint.clone(),
                    PendingTrust {
                        prompt_id,
                        presented: presented.clone(),
                        outcome: sender,
                        event_senders: vec![(prompt_id, events.clone())],
                        deciding: false,
                    },
                );
                self.set_awaiting(&endpoint, true);
                (prompt_id, receiver, true, prompt_id)
            }
        };

        let _guard = PendingTrustGuard {
            coordinator: self,
            endpoint: endpoint.clone(),
            prompt_id,
            owner: is_owner,
            waiter_token,
        };

        if is_owner
            && events
                .send(SessionEvent::UnknownHostKey {
                    prompt_id,
                    endpoint: endpoint.clone(),
                    algorithm: presented.algorithm,
                    fingerprint_sha256: presented.fingerprint_sha256,
                })
                .await
                .is_err()
        {
            let _ = self.decide(prompt_id, HostKeyDecision::Reject);
            return Err(SessionError::Disconnected);
        }

        loop {
            match *outcome.borrow() {
                TrustOutcome::Accepted => return Ok(()),
                TrustOutcome::Rejected => return Err(SessionError::HostKeyRejected),
                TrustOutcome::Conflict => return Err(SessionError::HostKeyChanged),
                TrustOutcome::Waiting => {}
            }
            tokio::select! {
                changed = outcome.changed() => {
                    if changed.is_err() {
                        return Err(SessionError::HostKeyRejected);
                    }
                }
                () = cancellation.cancelled() => {
                    let _ = self.decide(prompt_id, HostKeyDecision::Reject);
                    return Err(SessionError::Disconnected);
                }
            }
        }
    }

    fn decide(&self, prompt_id: Uuid, decision: HostKeyDecision) -> Result<(), SessionError> {
        let mut pending = self.pending.lock();
        let Some((endpoint, state)) = pending
            .iter_mut()
            .find(|(_, state)| state.prompt_id == prompt_id)
        else {
            return Err(SessionError::HostKeyRejected);
        };
        if state.deciding {
            return Err(SessionError::HostKeyRejected);
        }
        state.deciding = true;
        let endpoint = endpoint.clone();
        let presented = state.presented.clone();

        if decision == HostKeyDecision::AcceptAndStore {
            let accepted_at_unix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .try_into()
                .unwrap_or(i64::MAX);
            match self.store.accept_host_key(KnownHostEntry {
                host: endpoint.host.clone(),
                port: endpoint.port,
                algorithm: presented.algorithm,
                public_key: presented.public_key,
                fingerprint_sha256: presented.fingerprint_sha256,
                accepted_at_unix,
            }) {
                Ok(()) => {}
                Err(StorageError::HostKeyConflict) => {
                    let state = pending.remove(&endpoint).expect("pending entry present");
                    state.outcome.send_replace(TrustOutcome::Conflict);
                    drop(pending);
                    self.set_awaiting(&endpoint, false);
                    return Err(SessionError::HostKeyChanged);
                }
                Err(_) => {
                    let state = pending.remove(&endpoint).expect("pending entry present");
                    state.outcome.send_replace(TrustOutcome::Rejected);
                    drop(pending);
                    self.set_awaiting(&endpoint, false);
                    return Err(SessionError::HostKeyStoreFailed);
                }
            }
        }

        let state = pending.remove(&endpoint).expect("pending entry present");
        state.outcome.send_replace(match decision {
            HostKeyDecision::AcceptAndStore => TrustOutcome::Accepted,
            HostKeyDecision::Reject => TrustOutcome::Rejected,
        });
        drop(pending);
        self.set_awaiting(&endpoint, false);
        Ok(())
    }
}

struct PendingTrustGuard<'a> {
    coordinator: &'a HostTrustCoordinator,
    endpoint: Endpoint,
    prompt_id: Uuid,
    owner: bool,
    waiter_token: Uuid,
}

impl Drop for PendingTrustGuard<'_> {
    fn drop(&mut self) {
        let mut pending = self.coordinator.pending.lock();
        let Some(state) = pending.get_mut(&self.endpoint) else {
            return;
        };
        if state.prompt_id != self.prompt_id {
            return;
        }
        if self.owner {
            let state = pending
                .remove(&self.endpoint)
                .expect("pending entry present");
            state.outcome.send_replace(TrustOutcome::Rejected);
            drop(pending);
            self.coordinator.set_awaiting(&self.endpoint, false);
        } else {
            state
                .event_senders
                .retain(|(token, _)| *token != self.waiter_token);
        }
    }
}

fn same_presented_key(left: &PresentedHostKey, right: &PresentedHostKey) -> bool {
    left.algorithm == right.algorithm
        && left.public_key == right.public_key
        && left.fingerprint_sha256 == right.fingerprint_sha256
}

struct ClientHandler {
    endpoint: Endpoint,
    events: Sender<SessionEvent>,
    cancellation: CancellationToken,
    trust: Arc<HostTrustCoordinator>,
}

impl client::Handler for ClientHandler {
    type Error = SessionError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        self.events
            .send(SessionEvent::StateChanged(SessionState::VerifyingHostKey))
            .await
            .map_err(|_| SessionError::Disconnected)?;
        let presented = PresentedHostKey {
            algorithm: server_public_key.algorithm().to_string(),
            public_key: server_public_key
                .to_openssh()
                .map_err(|_| SessionError::ConnectFailed)?,
            fingerprint_sha256: server_public_key.fingerprint(HashAlg::Sha256).to_string(),
        };
        self.trust
            .verify(
                &self.endpoint,
                presented,
                self.events.clone(),
                self.cancellation.clone(),
            )
            .await?;
        Ok(true)
    }
}

type DynamicAgent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;
type AgentConnectFuture = Pin<Box<dyn Future<Output = Result<DynamicAgent, SessionError>> + Send>>;

trait AgentConnector: Send + Sync {
    fn connect(&self) -> AgentConnectFuture;
}

struct SystemAgentConnector;

impl AgentConnector for SystemAgentConnector {
    fn connect(&self) -> AgentConnectFuture {
        #[cfg(unix)]
        {
            Box::pin(async {
                AgentClient::connect_env()
                    .await
                    .map(AgentClient::dynamic)
                    .map_err(|_| SessionError::AgentUnavailable)
            })
        }
        #[cfg(windows)]
        {
            Box::pin(async {
                match AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
                    Ok(agent) => Ok(agent.dynamic()),
                    Err(_) => AgentClient::connect_pageant()
                        .await
                        .map(AgentClient::dynamic)
                        .map_err(|_| SessionError::AgentUnavailable),
                }
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Box::pin(async { Err(SessionError::AgentUnavailable) })
        }
    }
}

async fn run_session(
    request: ConnectRequest,
    input: Receiver<Bytes>,
    mut resize: watch::Receiver<TerminalSize>,
    events: Sender<SessionEvent>,
    cancellation: CancellationToken,
    trust: Arc<HostTrustCoordinator>,
    agent_connector: Arc<dyn AgentConnector>,
) -> Result<Option<u32>, SessionError> {
    let endpoint = canonicalize_endpoint(&request.profile.endpoint)
        .map_err(|_| SessionError::InvalidProfile)?;
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });
    let handler = ClientHandler {
        endpoint: endpoint.clone(),
        events: events.clone(),
        cancellation: cancellation.clone(),
        trust: trust.clone(),
    };
    let address = (endpoint.host.as_str(), endpoint.port);
    let mut connect = Box::pin(client::connect(config, address, handler));
    let mut awaiting = trust.subscribe_awaiting(&endpoint);
    let mut deadline = Box::pin(tokio::time::sleep(CONNECT_TIMEOUT));
    let mut handle = loop {
        tokio::select! {
            result = &mut connect => break result?,
            _ = &mut deadline => return Err(SessionError::ConnectTimeout),
            changed = awaiting.changed() => {
                if changed.is_err() {
                    return Err(SessionError::ConnectTimeout);
                }
                deadline = Box::pin(tokio::time::sleep(if *awaiting.borrow() {
                    HOST_KEY_DECISION_TIMEOUT
                } else {
                    CONNECT_TIMEOUT
                }));
            }
        }
    };

    events
        .send(SessionEvent::StateChanged(SessionState::Authenticating))
        .await
        .map_err(|_| SessionError::Disconnected)?;
    authenticate(
        &mut handle,
        &request.profile,
        request.secret.as_ref(),
        agent_connector.as_ref(),
    )
    .await?;
    drop(request.secret);

    events
        .send(SessionEvent::StateChanged(SessionState::OpeningShell))
        .await
        .map_err(|_| SessionError::Disconnected)?;
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|_| SessionError::ConnectFailed)?;
    channel
        .request_pty(
            true,
            "xterm-256color",
            request.initial_size.columns,
            request.initial_size.rows,
            request.initial_size.pixel_width,
            request.initial_size.pixel_height,
            &[],
        )
        .await
        .map_err(|_| SessionError::PtyRejected)?;
    channel
        .request_shell(true)
        .await
        .map_err(|_| SessionError::ShellRejected)?;
    events
        .send(SessionEvent::StateChanged(SessionState::Connected))
        .await
        .map_err(|_| SessionError::Disconnected)?;

    let mut exit_status = None;
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                let _ = channel.close().await;
                let _ = handle.disconnect(Disconnect::ByApplication, "", "").await;
                return Err(SessionError::Disconnected);
            }
            next_input = input.recv() => {
                let bytes = next_input.map_err(|_| SessionError::Disconnected)?;
                channel.data_bytes(bytes).await.map_err(|_| SessionError::Disconnected)?;
            }
            changed = resize.changed() => {
                if changed.is_err() {
                    return Err(SessionError::Disconnected);
                }
                let size = *resize.borrow_and_update();
                channel.window_change(
                    size.columns,
                    size.rows,
                    size.pixel_width,
                    size.pixel_height,
                ).await.map_err(|_| SessionError::Disconnected)?;
            }
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        events.send(SessionEvent::Output(data)).await
                            .map_err(|_| SessionError::Disconnected)?;
                    }
                    Some(ChannelMsg::ExitStatus { exit_status: status }) => {
                        exit_status = Some(status);
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        return Ok(exit_status);
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

pub fn private_key_requires_passphrase(path: &std::path::Path) -> Result<bool, SessionError> {
    if std::fs::File::open(path).is_err() {
        return Err(SessionError::PrivateKeyUnreadable);
    }
    match russh::keys::load_secret_key(path, None) {
        Ok(_) => Ok(false),
        Err(russh::keys::Error::KeyIsEncrypted) => Ok(true),
        Err(_) => Err(SessionError::PrivateKeyUnreadable),
    }
}

async fn authenticate<H: client::Handler<Error = SessionError>>(
    handle: &mut client::Handle<H>,
    profile: &ConnectionProfile,
    secret: Option<&SecretString>,
    agent_connector: &dyn AgentConnector,
) -> Result<(), SessionError> {
    let accepted = match &profile.auth {
        AuthConfig::Password { .. } => {
            let secret = secret.ok_or(SessionError::CredentialUnavailable)?;
            handle
                .authenticate_password(&profile.username, secret.expose_secret())
                .await
                .map_err(|_| SessionError::ConnectFailed)?
                .success()
        }
        AuthConfig::PrivateKey { path, .. } => {
            if std::fs::File::open(path).is_err() {
                return Err(SessionError::PrivateKeyUnreadable);
            }
            let passphrase = secret.map(ExposeSecret::expose_secret);
            let private_key = russh::keys::load_secret_key(path, passphrase).map_err(|error| {
                if matches!(error, russh::keys::Error::KeyIsEncrypted) && passphrase.is_none() {
                    SessionError::PrivateKeyPassphraseRequired
                } else if passphrase.is_some() {
                    SessionError::PrivateKeyPassphraseRejected
                } else {
                    SessionError::PrivateKeyUnreadable
                }
            })?;
            let hash_alg = if matches!(private_key.algorithm(), ssh_key::Algorithm::Rsa { .. }) {
                handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|_| SessionError::ConnectFailed)?
                    .flatten()
            } else {
                None
            };
            handle
                .authenticate_publickey(
                    &profile.username,
                    PrivateKeyWithHashAlg::new(Arc::new(private_key), hash_alg),
                )
                .await
                .map_err(|_| SessionError::ConnectFailed)?
                .success()
        }
        AuthConfig::Agent => {
            let mut agent = agent_connector.connect().await?;
            let identities = agent
                .request_identities()
                .await
                .map_err(|_| SessionError::AgentUnavailable)?;
            if identities.is_empty() {
                return Err(SessionError::AgentEmpty);
            }
            let mut authenticated = false;
            for identity in identities {
                let public_key = identity.public_key().into_owned();
                let hash_alg = if matches!(public_key.algorithm(), ssh_key::Algorithm::Rsa { .. }) {
                    handle
                        .best_supported_rsa_hash()
                        .await
                        .map_err(|_| SessionError::ConnectFailed)?
                        .flatten()
                } else {
                    None
                };
                let result = handle
                    .authenticate_publickey_with(
                        &profile.username,
                        public_key,
                        hash_alg,
                        &mut agent,
                    )
                    .await
                    .map_err(|_| SessionError::AgentUnavailable)?;
                if result.success() {
                    authenticated = true;
                    break;
                }
            }
            authenticated
        }
    };

    if accepted {
        Ok(())
    } else {
        Err(SessionError::AuthenticationRejected)
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use async_channel::TryRecvError;
    use russh::{
        Channel, ChannelId, Pty,
        keys::ssh_key::{Algorithm, LineEnding, PrivateKey, PublicKey},
        server::{self, Server as _, Session},
    };
    use tempfile::{TempDir, tempdir};

    use super::*;
    fn runtime() -> Arc<Runtime> {
        Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .unwrap(),
        )
    }

    fn endpoint() -> Endpoint {
        Endpoint {
            host: "EXAMPLE.COM".into(),
            port: 22,
        }
    }

    fn presented(name: &str) -> PresentedHostKey {
        PresentedHostKey {
            algorithm: "ssh-ed25519".into(),
            public_key: format!("ssh-ed25519 {name}"),
            fingerprint_sha256: format!("SHA256:{name}"),
        }
    }

    fn trust_fixture() -> (tempfile::TempDir, Arc<AppStore>, Arc<HostTrustCoordinator>) {
        let directory = tempdir().unwrap();
        let store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let trust = Arc::new(HostTrustCoordinator::new(store.clone()));
        (directory, store, trust)
    }

    async fn unknown_prompt(receiver: &Receiver<SessionEvent>) -> Uuid {
        match receiver.recv().await.unwrap() {
            SessionEvent::UnknownHostKey { prompt_id, .. } => prompt_id,
            event => panic!("expected unknown host key prompt, got {event:?}"),
        }
    }

    #[test]
    fn unknown_host_requires_acceptance() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, _store, trust) = trust_fixture();
            let (events, receiver) = async_channel::bounded(8);
            let verify = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("one"),
                            events,
                            CancellationToken::new(),
                        )
                        .await
                }
            });

            let prompt_id = unknown_prompt(&receiver).await;
            assert!(!verify.is_finished());
            trust
                .decide(prompt_id, HostKeyDecision::AcceptAndStore)
                .unwrap();
            assert_eq!(verify.await.unwrap(), Ok(()));
        });
    }

    #[test]
    fn accepted_host_is_reused() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, _store, trust) = trust_fixture();
            let (events, receiver) = async_channel::bounded(8);
            let first = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("one"),
                            events,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            let prompt_id = unknown_prompt(&receiver).await;
            trust
                .decide(prompt_id, HostKeyDecision::AcceptAndStore)
                .unwrap();
            assert_eq!(first.await.unwrap(), Ok(()));

            let (events, receiver) = async_channel::bounded(8);
            assert_eq!(
                trust
                    .verify(
                        &endpoint(),
                        presented("one"),
                        events,
                        CancellationToken::new(),
                    )
                    .await,
                Ok(())
            );
            assert!(matches!(receiver.try_recv(), Err(TryRecvError::Closed)));
        });
    }

    #[test]
    fn changed_host_is_hard_rejected() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, store, trust) = trust_fixture();
            store
                .accept_host_key(KnownHostEntry {
                    host: endpoint().host,
                    port: 22,
                    algorithm: "ssh-ed25519".into(),
                    public_key: "ssh-ed25519 one".into(),
                    fingerprint_sha256: "SHA256:one".into(),
                    accepted_at_unix: 1,
                })
                .unwrap();
            let (events, receiver) = async_channel::bounded(8);

            assert_eq!(
                trust
                    .verify(
                        &endpoint(),
                        presented("two"),
                        events,
                        CancellationToken::new(),
                    )
                    .await,
                Err(SessionError::HostKeyChanged)
            );
            assert!(matches!(
                receiver.recv().await.unwrap(),
                SessionEvent::ChangedHostKey { .. }
            ));
        });
    }

    #[test]
    fn deleting_trust_allows_a_new_prompt() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, store, trust) = trust_fixture();
            store
                .accept_host_key(KnownHostEntry {
                    host: endpoint().host,
                    port: 22,
                    algorithm: "ssh-ed25519".into(),
                    public_key: "ssh-ed25519 one".into(),
                    fingerprint_sha256: "SHA256:one".into(),
                    accepted_at_unix: 1,
                })
                .unwrap();
            store.delete_known_host(&endpoint()).unwrap();
            let (events, receiver) = async_channel::bounded(8);
            let verify = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("two"),
                            events,
                            CancellationToken::new(),
                        )
                        .await
                }
            });

            let prompt_id = unknown_prompt(&receiver).await;
            trust.decide(prompt_id, HostKeyDecision::Reject).unwrap();
            assert_eq!(verify.await.unwrap(), Err(SessionError::HostKeyRejected));
        });
    }

    #[test]
    fn concurrent_same_key_connections_share_one_prompt() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, _store, trust) = trust_fixture();
            let (events_one, receiver_one) = async_channel::bounded(8);
            let (events_two, receiver_two) = async_channel::bounded(8);
            let first = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("same"),
                            events_one,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            let prompt_id = unknown_prompt(&receiver_one).await;
            let second = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("same"),
                            events_two,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            tokio::task::yield_now().await;
            assert!(matches!(receiver_two.try_recv(), Err(TryRecvError::Empty)));

            trust
                .decide(prompt_id, HostKeyDecision::AcceptAndStore)
                .unwrap();
            assert_eq!(first.await.unwrap(), Ok(()));
            assert_eq!(second.await.unwrap(), Ok(()));
        });
    }

    #[test]
    fn concurrent_different_keys_are_rejected() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, _store, trust) = trust_fixture();
            let (events_one, receiver_one) = async_channel::bounded(8);
            let (events_two, receiver_two) = async_channel::bounded(8);
            let first = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("one"),
                            events_one,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            let _prompt_id = unknown_prompt(&receiver_one).await;
            let second = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("two"),
                            events_two,
                            CancellationToken::new(),
                        )
                        .await
                }
            });

            assert_eq!(second.await.unwrap(), Err(SessionError::HostKeyChanged));
            assert_eq!(first.await.unwrap(), Err(SessionError::HostKeyChanged));
            assert!(matches!(
                receiver_one.recv().await.unwrap(),
                SessionEvent::ChangedHostKey { .. }
            ));
            assert!(matches!(
                receiver_two.recv().await.unwrap(),
                SessionEvent::ChangedHostKey { .. }
            ));
        });
    }

    #[test]
    fn dropped_pending_verification_allows_a_fresh_prompt() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, _store, trust) = trust_fixture();
            let (events, receiver) = async_channel::bounded(8);
            let first = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("one"),
                            events,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            let first_prompt = unknown_prompt(&receiver).await;

            // Simulate the connect-timeout path: the verification future is
            // dropped while awaiting the decision.
            first.abort();
            assert!(first.await.is_err());

            // The stale pending entry must be gone: a retry with the same key
            // becomes the owner and emits a fresh prompt instead of joining
            // the ghost entry as a silent waiter.
            let (events, receiver) = async_channel::bounded(8);
            let second = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("one"),
                            events,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            let second_prompt = unknown_prompt(&receiver).await;
            assert_ne!(first_prompt, second_prompt);

            trust
                .decide(second_prompt, HostKeyDecision::AcceptAndStore)
                .unwrap();
            assert_eq!(second.await.unwrap(), Ok(()));
        });
    }

    #[test]
    fn abandoned_same_key_waiter_is_removed_from_the_pending_entry() {
        let runtime = runtime();
        runtime.block_on(async {
            let (_directory, _store, trust) = trust_fixture();
            let (events_one, receiver_one) = async_channel::bounded(8);
            let first = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("same"),
                            events_one,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            let prompt_id = unknown_prompt(&receiver_one).await;

            let (events_two, receiver_two) = async_channel::bounded(8);
            let second = tokio::spawn({
                let trust = trust.clone();
                async move {
                    trust
                        .verify(
                            &endpoint(),
                            presented("same"),
                            events_two,
                            CancellationToken::new(),
                        )
                        .await
                }
            });
            tokio::task::yield_now().await;
            second.abort();
            assert!(second.await.is_err());

            trust
                .decide(prompt_id, HostKeyDecision::AcceptAndStore)
                .unwrap();
            assert_eq!(first.await.unwrap(), Ok(()));
            // The abandoned waiter's sender was detached from the entry; only
            // the owner's channel receives the terminal event stream.
            assert!(receiver_two.is_closed());
        });
    }

    fn test_handle() -> (SessionHandle, Receiver<Bytes>) {
        let runtime = runtime();
        let directory = tempdir().unwrap();
        let store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let trust = Arc::new(HostTrustCoordinator::new(store));
        let (events_sender, events_receiver) = async_channel::bounded(8);
        let (input, input_receiver) = async_channel::bounded(INPUT_QUEUE_CAPACITY);
        let (resize, _resize_receiver) = watch::channel(TerminalSize {
            columns: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 480,
        });
        let cancellation = CancellationToken::new();
        let task = runtime.spawn(std::future::pending::<()>());
        drop(events_sender);
        drop(directory);
        (
            SessionHandle {
                id: SessionId::new(),
                events: Some(events_receiver),
                input,
                resize,
                cancellation,
                task_abort: task.abort_handle(),
                runtime,
                trust,
                disconnect_started: AtomicBool::new(false),
            },
            input_receiver,
        )
    }

    #[test]
    fn session_events_can_only_be_taken_once() {
        let (mut handle, _input_receiver) = test_handle();

        assert!(handle.take_events().is_some());
        assert!(handle.take_events().is_none());
    }

    #[test]
    fn input_backpressure_is_reported() {
        let (handle, _input_receiver) = test_handle();
        for _ in 0..INPUT_QUEUE_CAPACITY {
            assert_eq!(handle.try_send_input(Bytes::from_static(b"x")), Ok(()));
        }

        assert_eq!(
            handle.try_send_input(Bytes::from_static(b"x")),
            Err(SendInputError::QueueFull)
        );
    }

    #[derive(Clone)]
    struct FixtureServer {
        accepted_keys: Arc<Vec<PublicKey>>,
        resize_events: Sender<TerminalSize>,
    }

    impl server::Server for FixtureServer {
        type Handler = Self;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl server::Handler for FixtureServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> Result<server::Auth, Self::Error> {
            if user == "oxide" && password == "oxide-test" {
                Ok(server::Auth::Accept)
            } else {
                Ok(server::Auth::reject())
            }
        }

        async fn auth_publickey(
            &mut self,
            user: &str,
            key: &PublicKey,
        ) -> Result<server::Auth, Self::Error> {
            if user == "oxide" && self.accepted_keys.iter().any(|accepted| accepted == key) {
                Ok(server::Auth::Accept)
            } else {
                Ok(server::Auth::reject())
            }
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            col_width: u32,
            row_height: u32,
            pix_width: u32,
            pix_height: u32,
            _modes: &[(Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = self.resize_events.try_send(TerminalSize {
                columns: col_width,
                rows: row_height,
                pixel_width: pix_width,
                pixel_height: pix_height,
            });
            session.channel_success(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            Ok(())
        }

        async fn window_change_request(
            &mut self,
            _channel: ChannelId,
            col_width: u32,
            row_height: u32,
            pix_width: u32,
            pix_height: u32,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = self.resize_events.try_send(TerminalSize {
                columns: col_width,
                rows: row_height,
                pixel_width: pix_width,
                pixel_height: pix_height,
            });
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.data(channel, data.to_vec())?;
            Ok(())
        }
    }

    struct SshFixture {
        _directory: TempDir,
        port: u16,
        key_path: PathBuf,
        #[cfg(unix)]
        agent_key: PrivateKey,
        resize_events: Receiver<TerminalSize>,
        server_task: tokio::task::JoinHandle<()>,
    }

    impl Drop for SshFixture {
        fn drop(&mut self) {
            self.server_task.abort();
        }
    }

    async fn start_ssh_fixture() -> SshFixture {
        let directory = tempdir().unwrap();
        let file_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        #[cfg(unix)]
        let agent_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        #[cfg(unix)]
        let accepted_keys = vec![
            file_key.public_key().clone(),
            agent_key.public_key().clone(),
        ];
        #[cfg(not(unix))]
        let accepted_keys = vec![file_key.public_key().clone()];
        let accepted_keys = Arc::new(accepted_keys);
        let encrypted = file_key
            .encrypt(&mut rand::rng(), "oxide-key-test")
            .unwrap();
        let key_path = directory.path().join("id_ed25519");
        std::fs::write(
            &key_path,
            encrypted.to_openssh(LineEnding::LF).unwrap().as_bytes(),
        )
        .unwrap();

        let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let config = Arc::new(server::Config {
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            keys: vec![host_key],
            ..Default::default()
        });
        let (resize_sender, resize_events) = async_channel::bounded(16);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture_server = FixtureServer {
            accepted_keys,
            resize_events: resize_sender,
        };
        let server_task = tokio::spawn(async move {
            let mut server = fixture_server;
            let _ = server.run_on_socket(config, &listener).await;
        });

        SshFixture {
            _directory: directory,
            port,
            key_path,
            #[cfg(unix)]
            agent_key,
            resize_events,
            server_task,
        }
    }

    fn connection_profile(port: u16, auth: AuthConfig) -> ConnectionProfile {
        ConnectionProfile {
            id: crate::model::ProfileId::new(),
            name: "Local fixture".into(),
            endpoint: Endpoint {
                host: "127.0.0.1".into(),
                port,
            },
            username: "oxide".into(),
            auth,
        }
    }

    async fn wait_for_connected(
        handle: &SessionHandle,
        events: &Receiver<SessionEvent>,
    ) -> Result<(), SessionError> {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .map_err(|_| SessionError::ConnectTimeout)?
                .map_err(|_| SessionError::Disconnected)?;
            match event {
                SessionEvent::UnknownHostKey { prompt_id, .. } => {
                    handle.decide_host_key(prompt_id, HostKeyDecision::AcceptAndStore)?;
                }
                SessionEvent::StateChanged(SessionState::Connected) => return Ok(()),
                SessionEvent::Error(error) => return Err(error),
                SessionEvent::Exited { .. }
                | SessionEvent::StateChanged(SessionState::Disconnected) => {
                    return Err(SessionError::Disconnected);
                }
                _ => {}
            }
        }
    }

    #[cfg(unix)]
    struct SocketAgentConnector {
        socket: PathBuf,
    }

    #[cfg(unix)]
    impl AgentConnector for SocketAgentConnector {
        fn connect(&self) -> AgentConnectFuture {
            let socket = self.socket.clone();
            Box::pin(async move {
                AgentClient::connect_uds(socket)
                    .await
                    .map(AgentClient::dynamic)
                    .map_err(|_| SessionError::AgentUnavailable)
            })
        }
    }

    #[cfg(unix)]
    async fn start_test_agent(
        key: &PrivateKey,
    ) -> (
        TempDir,
        Arc<dyn AgentConnector>,
        tokio::task::JoinHandle<()>,
    ) {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("agent.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let incoming = futures::stream::unfold(listener, |listener| async move {
            let next = listener.accept().await.map(|(stream, _)| stream);
            Some((next, listener))
        });
        let task = tokio::spawn(async move {
            let _ = russh::keys::agent::server::serve(Box::pin(incoming), ()).await;
        });
        let mut seeder = AgentClient::connect_uds(&socket).await.unwrap();
        seeder.add_identity(key, &[]).await.unwrap();
        (directory, Arc::new(SocketAgentConnector { socket }), task)
    }

    #[cfg(unix)]
    #[test]
    fn password_private_key_and_signer_authenticate() {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        );
        let (password_service, agent_service) = runtime.block_on(async {
            let fixture = start_ssh_fixture().await;
            let store_directory = tempdir().unwrap();
            let store = Arc::new(AppStore::open(store_directory.path().to_path_buf()).unwrap());
            let password_service = SshService::new(store.clone()).unwrap();

            let password_profile = connection_profile(
                fixture.port,
                AuthConfig::Password {
                    credential_ref: None,
                },
            );
            let mut password = password_service
                .connect(ConnectRequest {
                    profile: password_profile,
                    secret: Some(SecretString::from("oxide-test")),
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let password_events = password.take_events().unwrap();
            wait_for_connected(&password, &password_events)
                .await
                .unwrap();
            password
                .try_send_input(Bytes::from_static(b"oxide-ok"))
                .unwrap();
            loop {
                if let SessionEvent::Output(output) = password_events.recv().await.unwrap() {
                    assert_eq!(output, Bytes::from_static(b"oxide-ok"));
                    break;
                }
            }
            password.disconnect();

            let key_profile = connection_profile(
                fixture.port,
                AuthConfig::PrivateKey {
                    path: fixture.key_path.clone(),
                    passphrase_ref: None,
                },
            );
            let mut key_session = password_service
                .connect(ConnectRequest {
                    profile: key_profile,
                    secret: Some(SecretString::from("oxide-key-test")),
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let key_events = key_session.take_events().unwrap();
            wait_for_connected(&key_session, &key_events).await.unwrap();
            key_session.disconnect();

            let (_agent_directory, connector, agent_task) =
                start_test_agent(&fixture.agent_key).await;
            let agent_service = SshService::with_agent_connector(store, connector).unwrap();
            let agent_profile = connection_profile(fixture.port, AuthConfig::Agent);
            let mut agent_session = agent_service
                .connect(ConnectRequest {
                    profile: agent_profile,
                    secret: None,
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let agent_events = agent_session.take_events().unwrap();
            wait_for_connected(&agent_session, &agent_events)
                .await
                .unwrap();
            agent_session.disconnect();
            agent_task.abort();
            (password_service, agent_service)
        });
        drop(password_service);
        drop(agent_service);
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let runtime = runtime();
        let service = runtime.block_on(async {
            let fixture = start_ssh_fixture().await;
            let store_directory = tempdir().unwrap();
            let store = Arc::new(AppStore::open(store_directory.path().to_path_buf()).unwrap());
            let service = SshService::new(store).unwrap();
            let profile = connection_profile(
                fixture.port,
                AuthConfig::Password {
                    credential_ref: None,
                },
            );
            let mut session = service
                .connect(ConnectRequest {
                    profile,
                    secret: Some(SecretString::from("wrong")),
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let events = session.take_events().unwrap();
            loop {
                match events.recv().await.unwrap() {
                    SessionEvent::UnknownHostKey { prompt_id, .. } => session
                        .decide_host_key(prompt_id, HostKeyDecision::AcceptAndStore)
                        .unwrap(),
                    SessionEvent::Error(error) => {
                        assert_eq!(error, SessionError::AuthenticationRejected);
                        break;
                    }
                    _ => {}
                }
            }
            service
        });
        drop(service);
    }

    #[test]
    fn wrong_private_key_passphrase_is_distinct() {
        let runtime = runtime();
        let service = runtime.block_on(async {
            let fixture = start_ssh_fixture().await;
            let store_directory = tempdir().unwrap();
            let store = Arc::new(AppStore::open(store_directory.path().to_path_buf()).unwrap());
            let service = SshService::new(store).unwrap();
            let profile = connection_profile(
                fixture.port,
                AuthConfig::PrivateKey {
                    path: fixture.key_path.clone(),
                    passphrase_ref: None,
                },
            );
            let mut missing_secret_session = service
                .connect(ConnectRequest {
                    profile: profile.clone(),
                    secret: None,
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let missing_secret_events = missing_secret_session.take_events().unwrap();
            loop {
                match missing_secret_events.recv().await.unwrap() {
                    SessionEvent::UnknownHostKey { prompt_id, .. } => missing_secret_session
                        .decide_host_key(prompt_id, HostKeyDecision::AcceptAndStore)
                        .unwrap(),
                    SessionEvent::Error(error) => {
                        assert_eq!(error, SessionError::PrivateKeyPassphraseRequired);
                        break;
                    }
                    _ => {}
                }
            }
            let mut session = service
                .connect(ConnectRequest {
                    profile,
                    secret: Some(SecretString::from("wrong-key-passphrase")),
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let events = session.take_events().unwrap();
            loop {
                match events.recv().await.unwrap() {
                    SessionEvent::UnknownHostKey { prompt_id, .. } => session
                        .decide_host_key(prompt_id, HostKeyDecision::AcceptAndStore)
                        .unwrap(),
                    SessionEvent::Error(error) => {
                        assert_eq!(error, SessionError::PrivateKeyPassphraseRejected);
                        break;
                    }
                    _ => {}
                }
            }
            service
        });
        drop(service);
    }
    #[test]
    fn pty_resize_reaches_server() {
        let runtime = runtime();
        let service = runtime.block_on(async {
            let fixture = start_ssh_fixture().await;
            let store_directory = tempdir().unwrap();
            let store = Arc::new(AppStore::open(store_directory.path().to_path_buf()).unwrap());
            let service = SshService::new(store).unwrap();
            let profile = connection_profile(
                fixture.port,
                AuthConfig::Password {
                    credential_ref: None,
                },
            );
            let mut session = service
                .connect(ConnectRequest {
                    profile,
                    secret: Some(SecretString::from("oxide-test")),
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let events = session.take_events().unwrap();
            wait_for_connected(&session, &events).await.unwrap();
            let expected = TerminalSize {
                columns: 132,
                rows: 43,
                pixel_width: 1320,
                pixel_height: 860,
            };
            session.resize(expected);

            loop {
                let observed =
                    tokio::time::timeout(Duration::from_secs(5), fixture.resize_events.recv())
                        .await
                        .unwrap()
                        .unwrap();
                if observed == expected {
                    break;
                }
            }
            session.disconnect();
            service
        });
        drop(service);
    }

    #[test]
    fn disconnect_terminates_once() {
        let runtime = runtime();
        let service = runtime.block_on(async {
            let fixture = start_ssh_fixture().await;
            let store_directory = tempdir().unwrap();
            let store = Arc::new(AppStore::open(store_directory.path().to_path_buf()).unwrap());
            let service = SshService::new(store).unwrap();
            let profile = connection_profile(
                fixture.port,
                AuthConfig::Password {
                    credential_ref: None,
                },
            );
            let mut session = service
                .connect(ConnectRequest {
                    profile,
                    secret: Some(SecretString::from("oxide-test")),
                    initial_size: TerminalSize {
                        columns: 80,
                        rows: 24,
                        pixel_width: 800,
                        pixel_height: 480,
                    },
                })
                .unwrap();
            let events = session.take_events().unwrap();
            wait_for_connected(&session, &events).await.unwrap();

            session.disconnect();
            session.disconnect();
            let mut disconnected_events = 0;
            while let Ok(event) = tokio::time::timeout(Duration::from_secs(3), events.recv()).await
            {
                match event {
                    Ok(SessionEvent::StateChanged(SessionState::Disconnected)) => {
                        disconnected_events += 1;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            assert_eq!(disconnected_events, 1);
            service
        });
        drop(service);
    }
}
