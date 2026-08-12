use std::{sync::Arc, time::Duration};

use async_channel::{Sender, bounded};
use russh::{
    Channel, ChannelId,
    keys::ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey},
    server::{self, Msg, Server as _, Session},
};
use tempfile::tempdir;

#[derive(Clone)]
struct FixtureServer {
    accepted_key: Arc<PublicKey>,
    resize_events: Sender<(u32, u32, u32, u32)>,
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
        if user == "oxide" && key == self.accepted_key.as_ref() {
            Ok(server::Auth::Accept)
        } else {
            Ok(server::Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
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
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = self
            .resize_events
            .try_send((columns, rows, pixel_width, pixel_height));
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
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = self
            .resize_events
            .try_send((columns, rows, pixel_width, pixel_height));
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

fn requested_port() -> u16 {
    let mut args = std::env::args().skip(1);
    let mut port = 2222;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--port" => {
                port = args
                    .next()
                    .expect("--port requires a value")
                    .parse()
                    .expect("--port must be between 0 and 65535");
            }
            _ => panic!("unsupported argument: {argument}"),
        }
    }
    port
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create Tokio runtime");
    runtime.block_on(async {
        let directory = tempdir().expect("failed to create fixture directory");
        let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .expect("failed to generate client key");
        let encrypted_client_key = client_key
            .clone()
            .encrypt(&mut rand::rng(), "oxide-key-test")
            .expect("failed to encrypt client key");
        let client_key_path = directory.path().join("id_ed25519");
        std::fs::write(
            &client_key_path,
            encrypted_client_key
                .to_openssh(LineEnding::LF)
                .expect("failed to encode client key")
                .as_bytes(),
        )
        .expect("failed to write client key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&client_key_path, std::fs::Permissions::from_mode(0o600))
                .expect("failed to protect client key");
        }

        let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .expect("failed to generate host key");
        let host_algorithm = host_key.public_key().algorithm();
        let host_fingerprint = host_key.public_key().fingerprint(HashAlg::Sha256);
        let config = Arc::new(server::Config {
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            keys: vec![host_key],
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", requested_port()))
            .await
            .expect("failed to bind fixture server");
        let port = listener
            .local_addr()
            .expect("fixture listener has no local address")
            .port();
        let (resize_sender, resize_receiver) = bounded(32);
        let resize_logger = tokio::spawn(async move {
            while let Ok((columns, rows, pixel_width, pixel_height)) = resize_receiver.recv().await {
                println!(
                    "terminal_size columns={columns} rows={rows} pixel_width={pixel_width} pixel_height={pixel_height}"
                );
            }
        });

        println!("OxideSSH fixture server ready");
        println!("host=127.0.0.1");
        println!("port={port}");
        println!("username=oxide");
        println!("password=oxide-test");
        println!("private_key={}", client_key_path.display());
        println!("private_key_passphrase=oxide-key-test");
        println!("host_algorithm={host_algorithm}");
        println!("host_fingerprint_sha256={host_fingerprint}");

        let mut server = FixtureServer {
            accepted_key: Arc::new(client_key.public_key().clone()),
            resize_events: resize_sender,
        };
        let result = server.run_on_socket(config, &listener).await;
        resize_logger.abort();
        result.expect("fixture server failed");
    });
}
