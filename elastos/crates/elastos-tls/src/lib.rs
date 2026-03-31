//! TLS certificate management for the ElastOS runtime
//!
//! Generates a self-signed CA on first boot and creates leaf certificates
//! signed by that CA. The user trusts the CA once per device, then HTTPS
//! works from any transport (localhost, LAN, Tailscale, Tor, Boson).

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, SanType};

/// TLS file paths within the data directory
pub struct TlsPaths {
    pub ca_cert: PathBuf,
    pub ca_key: PathBuf,
    pub leaf_cert: PathBuf,
    pub leaf_key: PathBuf,
}

impl TlsPaths {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            ca_cert: data_dir.join("ca.pem"),
            ca_key: data_dir.join("ca.key"),
            leaf_cert: data_dir.join("tls.pem"),
            leaf_key: data_dir.join("tls.key"),
        }
    }
}

/// Load or create the TLS configuration.
///
/// Returns an `axum_server::tls_rustls::RustlsConfig` ready to use.
/// On first boot, generates a CA and leaf cert. On subsequent boots,
/// reuses the CA and regenerates the leaf cert if IPs changed.
pub async fn load_or_create_tls_config(
    data_dir: &Path,
) -> anyhow::Result<axum_server::tls_rustls::RustlsConfig> {
    fs::create_dir_all(data_dir)?;
    let paths = TlsPaths::new(data_dir);

    // Ensure CA exists
    let (ca_cert_pem, ca_key_pem) = ensure_ca(&paths)?;

    // Detect current IPs for SANs
    let ips = detect_ips();
    tracing::info!(
        "Detected IPs for TLS cert: {:?}",
        ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>()
    );

    // Check if leaf cert needs regeneration
    let needs_regen = if paths.leaf_cert.exists() && paths.leaf_key.exists() {
        needs_regeneration(&paths.leaf_cert, &ips)
    } else {
        true
    };

    if needs_regen {
        tracing::info!("Generating TLS leaf certificate...");
        generate_leaf_cert(&paths, &ca_cert_pem, &ca_key_pem, &ips)?;
        // Store IPs for change detection
        let mut ip_strs: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
        ip_strs.sort();
        fs::write(paths.leaf_cert.with_extension("ips"), ip_strs.join(","))?;
        tracing::info!(
            "TLS leaf certificate written to {}",
            paths.leaf_cert.display()
        );
    } else {
        tracing::info!("Reusing existing TLS leaf certificate");
    }

    // Build rustls config from PEM files
    let config =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(&paths.leaf_cert, &paths.leaf_key)
            .await?;

    Ok(config)
}

/// Ensure the CA certificate and key exist. Creates them if not.
fn ensure_ca(paths: &TlsPaths) -> anyhow::Result<(String, String)> {
    if paths.ca_cert.exists() && paths.ca_key.exists() {
        let cert_pem = fs::read_to_string(&paths.ca_cert)?;
        let key_pem = fs::read_to_string(&paths.ca_key)?;
        tracing::info!("Loaded existing CA from {}", paths.ca_cert.display());
        return Ok((cert_pem, key_pem));
    }

    tracing::info!("Generating new ElastOS CA certificate...");

    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "ElastOS CA");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "ElastOS");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    // 10-year validity
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(3650);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    fs::write(&paths.ca_cert, &cert_pem)?;
    fs::write(&paths.ca_key, &key_pem)?;

    // Restrict CA key permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.ca_key, fs::Permissions::from_mode(0o600))?;
    }

    tracing::info!("CA certificate written to {}", paths.ca_cert.display());
    tracing::info!("Trust this CA on your devices: elastos tls trust");

    Ok((cert_pem, key_pem))
}

/// Generate a leaf certificate signed by the CA.
fn generate_leaf_cert(
    paths: &TlsPaths,
    ca_cert_pem: &str,
    ca_key_pem: &str,
    ips: &[IpAddr],
) -> anyhow::Result<()> {
    // Reconstruct CA from stored PEM
    let ca_key = KeyPair::from_pem(ca_key_pem)?;
    let ca_params = CertificateParams::from_ca_cert_pem(ca_cert_pem)?;
    let ca_cert = ca_params.self_signed(&ca_key)?;

    // Build leaf cert params
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "ElastOS Runtime");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "ElastOS");
    params.is_ca = IsCa::NoCa;

    // 1-year validity
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(365);

    // SANs: localhost + all detected IPs
    let mut sans = vec![SanType::DnsName("localhost".try_into()?)];
    for ip in ips {
        sans.push(SanType::IpAddress(*ip));
    }
    // Always include loopback
    let loopback_v4: IpAddr = "127.0.0.1".parse().unwrap();
    if !ips.contains(&loopback_v4) {
        sans.push(SanType::IpAddress(loopback_v4));
    }
    params.subject_alt_names = sans;

    let leaf_key = KeyPair::generate()?;
    let leaf_cert = params.signed_by(&leaf_key, &ca_cert, &ca_key)?;

    // Write leaf cert (include CA cert for chain)
    let leaf_pem = format!("{}{}", leaf_cert.pem(), ca_cert_pem);
    let leaf_key_pem = leaf_key.serialize_pem();

    fs::write(&paths.leaf_cert, &leaf_pem)?;
    fs::write(&paths.leaf_key, &leaf_key_pem)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.leaf_key, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Detect local IP addresses (LAN, Tailscale, etc.)
fn detect_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();

    // Parse /proc/net/fib_trie or use a simpler approach: read from ip command
    // For portability, try reading network interfaces
    if let Ok(output) = std::process::Command::new("hostname").arg("-I").output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for part in stdout.split_whitespace() {
                if let Ok(ip) = part.parse::<IpAddr>() {
                    ips.push(ip);
                }
            }
        }
    }

    // Fallback: at minimum include loopback
    if ips.is_empty() {
        ips.push("127.0.0.1".parse().unwrap());
    }

    ips
}

/// Check if the leaf cert SANs match the current IPs.
/// Returns true if regeneration is needed.
fn needs_regeneration(cert_path: &Path, current_ips: &[IpAddr]) -> bool {
    // Simple approach: read the cert PEM and check if each IP is present as a string
    let cert_pem = match fs::read_to_string(cert_path) {
        Ok(pem) => pem,
        Err(_) => return true,
    };

    // Parse the cert to check SANs would require x509-parser crate.
    // Simple heuristic: check if the IPs file exists and matches.
    // We'll store the IP list alongside the cert for easy comparison.
    let ips_path = cert_path.with_extension("ips");
    let stored_ips = match fs::read_to_string(&ips_path) {
        Ok(s) => s,
        Err(_) => return true,
    };

    let mut current: Vec<String> = current_ips.iter().map(|ip| ip.to_string()).collect();
    current.sort();
    let current_str = current.join(",");

    if stored_ips.trim() != current_str {
        return true;
    }

    // Also check PEM is not empty/corrupt
    !cert_pem.contains("BEGIN CERTIFICATE")
}

/// Print CA trust instructions
pub fn print_trust_instructions(data_dir: &Path) {
    let paths = TlsPaths::new(data_dir);

    if !paths.ca_cert.exists() {
        println!("No CA certificate found. Run 'elastos serve' first to generate one.");
        return;
    }

    let ca_path = paths.ca_cert.display();
    println!("ElastOS CA certificate: {}\n", ca_path);
    println!("To trust this CA on your devices:\n");
    println!("  Linux (Chrome/Chromium):");
    println!(
        "    certutil -d sql:$HOME/.pki/nssdb -A -t \"C,,\" -n \"ElastOS\" -i {}\n",
        ca_path
    );
    println!("  Linux (Firefox):");
    println!("    Settings → Privacy & Security → Certificates → View Certificates → Import\n");
    println!("  macOS:");
    println!("    sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {}\n", ca_path);
    println!("  Windows:");
    println!("    certutil -addstore -user Root {}\n", ca_path);
    println!("After trusting, HTTPS will work from any device on your network.");
}

/// Regenerate the leaf certificate (e.g., after IP change)
pub fn regenerate_leaf(data_dir: &Path) -> anyhow::Result<()> {
    let paths = TlsPaths::new(data_dir);

    if !paths.ca_cert.exists() || !paths.ca_key.exists() {
        anyhow::bail!("No CA certificate found. Run 'elastos serve' first to generate one.");
    }

    let ca_cert_pem = fs::read_to_string(&paths.ca_cert)?;
    let ca_key_pem = fs::read_to_string(&paths.ca_key)?;
    let ips = detect_ips();

    generate_leaf_cert(&paths, &ca_cert_pem, &ca_key_pem, &ips)?;

    // Store IPs for change detection
    let mut ip_strs: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    ip_strs.sort();
    fs::write(paths.leaf_cert.with_extension("ips"), ip_strs.join(","))?;

    println!("Leaf certificate regenerated with SANs:");
    println!("  localhost, 127.0.0.1");
    for ip in &ips {
        if ip.to_string() != "127.0.0.1" {
            println!("  {}", ip);
        }
    }

    Ok(())
}

/// TLS-terminating reverse proxy.
///
/// Accepts HTTPS connections on `listen_port` and forwards plaintext TCP
/// to `target_addr` (the VM's guest IP:port). This allows the browser to
/// access the VM over HTTPS without the VM knowing about TLS.
pub async fn start_tls_proxy(
    listen_port: u16,
    target_ip: &str,
    target_port: u16,
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<()> {
    use rustls_pemfile::{certs, pkcs8_private_keys};
    use std::io::BufReader;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::TlsAcceptor;

    let cert_file = fs::File::open(cert_path)?;
    let key_file = fs::File::open(key_path)?;

    let certs: Vec<_> = certs(&mut BufReader::new(cert_file)).collect::<Result<Vec<_>, _>>()?;
    let keys: Vec<_> =
        pkcs8_private_keys(&mut BufReader::new(key_file)).collect::<Result<Vec<_>, _>>()?;
    let key = keys
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No private key found"))?;

    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            certs.into_iter().collect(),
            tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(key),
        )?;

    let acceptor = TlsAcceptor::from(std::sync::Arc::new(config));
    let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_port)).await?;
    let target_addr: std::net::SocketAddr = format!("{}:{}", target_ip, target_port).parse()?;

    tracing::info!(
        "TLS proxy started: https://0.0.0.0:{} -> {}:{}",
        listen_port,
        target_ip,
        target_port
    );

    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!("TLS proxy accept error: {}", e);
                    continue;
                }
            };

            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let tls_stream = match acceptor.accept(stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!("TLS handshake failed from {}: {}", peer, e);
                        return;
                    }
                };

                let mut vm_stream = match TcpStream::connect(target_addr).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!("Failed to connect to VM: {}", e);
                        return;
                    }
                };

                let (mut tls_read, mut tls_write) = tokio::io::split(tls_stream);
                let (mut vm_read, mut vm_write) = vm_stream.split();

                let c2v = async {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = tls_read.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }
                        vm_write.write_all(&buf[..n]).await?;
                    }
                    vm_write.shutdown().await?;
                    Ok::<_, std::io::Error>(())
                };

                let v2c = async {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = vm_read.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }
                        tls_write.write_all(&buf[..n]).await?;
                    }
                    tls_write.shutdown().await?;
                    Ok::<_, std::io::Error>(())
                };

                tokio::select! {
                    r = c2v => { if let Err(e) = r { tracing::trace!("c2v: {}", e); } }
                    r = v2c => { if let Err(e) = r { tracing::trace!("v2c: {}", e); } }
                }
            });
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_ips_returns_at_least_loopback() {
        let ips = detect_ips();
        assert!(!ips.is_empty());
    }

    #[test]
    fn test_tls_paths() {
        let paths = TlsPaths::new(Path::new("/tmp/test"));
        assert_eq!(paths.ca_cert, PathBuf::from("/tmp/test/ca.pem"));
        assert_eq!(paths.ca_key, PathBuf::from("/tmp/test/ca.key"));
        assert_eq!(paths.leaf_cert, PathBuf::from("/tmp/test/tls.pem"));
        assert_eq!(paths.leaf_key, PathBuf::from("/tmp/test/tls.key"));
    }

    #[test]
    fn test_ca_generation_and_leaf_cert() {
        let dir = tempfile::tempdir().unwrap();
        let paths = TlsPaths::new(dir.path());

        // Generate CA
        let (ca_cert_pem, ca_key_pem) = ensure_ca(&paths).unwrap();
        assert!(ca_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(ca_key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(paths.ca_cert.exists());
        assert!(paths.ca_key.exists());

        // Generate leaf cert
        let ips = vec![
            "127.0.0.1".parse().unwrap(),
            "192.168.1.100".parse().unwrap(),
        ];
        generate_leaf_cert(&paths, &ca_cert_pem, &ca_key_pem, &ips).unwrap();
        assert!(paths.leaf_cert.exists());
        assert!(paths.leaf_key.exists());

        // Leaf cert should contain both leaf and CA certs
        let leaf_pem = fs::read_to_string(&paths.leaf_cert).unwrap();
        assert_eq!(leaf_pem.matches("BEGIN CERTIFICATE").count(), 2);
    }

    #[test]
    fn test_ca_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let paths = TlsPaths::new(dir.path());

        let (pem1, key1) = ensure_ca(&paths).unwrap();
        let (pem2, key2) = ensure_ca(&paths).unwrap();

        // Should reuse same CA
        assert_eq!(pem1, pem2);
        assert_eq!(key1, key2);
    }
}
