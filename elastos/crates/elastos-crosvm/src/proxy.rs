//! Simple TCP proxy for port forwarding to VMs
//!
//! This provides a reliable way to forward ports from the host to the VM
//! without requiring complex iptables rules.

use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

/// TCP proxy that forwards connections from host to VM
pub struct TcpProxy {
    host_port: u16,
    vm_addr: SocketAddr,
    shutdown_tx: Option<broadcast::Sender<()>>,
}

impl TcpProxy {
    /// Create a new TCP proxy
    pub fn new(host_port: u16, vm_ip: &str, vm_port: u16) -> Self {
        let vm_addr: SocketAddr = format!("{}:{}", vm_ip, vm_port)
            .parse()
            .expect("Invalid VM address");

        Self {
            host_port,
            vm_addr,
            shutdown_tx: None,
        }
    }

    /// Start the proxy (spawns background task)
    pub async fn start(&mut self) -> Result<(), std::io::Error> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.host_port)).await?;
        let vm_addr = self.vm_addr;
        let (shutdown_tx, _) = broadcast::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        tracing::info!(
            "TCP proxy started: 0.0.0.0:{} -> {}",
            self.host_port,
            self.vm_addr
        );

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_tx.subscribe();

            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((client_stream, client_addr)) => {
                                tracing::debug!("New connection from {}", client_addr);
                                let vm_addr = vm_addr;
                                tokio::spawn(async move {
                                    if let Err(e) = proxy_connection(client_stream, vm_addr).await {
                                        tracing::debug!("Proxy connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::warn!("Accept error: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        tracing::info!("TCP proxy shutting down");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop the proxy
    pub fn stop(&self) {
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(());
        }
    }
}

/// Proxy a single connection between client and VM
async fn proxy_connection(
    mut client: TcpStream,
    vm_addr: SocketAddr,
) -> Result<(), std::io::Error> {
    // Connect to VM
    let mut vm_stream = match TcpStream::connect(vm_addr).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("Failed to connect to VM at {}: {}", vm_addr, e);
            return Err(e);
        }
    };

    let (mut client_read, mut client_write) = client.split();
    let (mut vm_read, mut vm_write) = vm_stream.split();

    // Bidirectional copy
    let client_to_vm = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = client_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            vm_write.write_all(&buf[..n]).await?;
        }
        vm_write.shutdown().await?;
        Ok::<_, std::io::Error>(())
    };

    let vm_to_client = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = vm_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            client_write.write_all(&buf[..n]).await?;
        }
        client_write.shutdown().await?;
        Ok::<_, std::io::Error>(())
    };

    // Run both directions concurrently
    tokio::select! {
        result = client_to_vm => {
            if let Err(e) = result {
                tracing::trace!("Client to VM error: {}", e);
            }
        }
        result = vm_to_client => {
            if let Err(e) = result {
                tracing::trace!("VM to client error: {}", e);
            }
        }
    }

    Ok(())
}
