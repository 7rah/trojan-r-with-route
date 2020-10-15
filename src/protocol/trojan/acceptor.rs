use crate::protocol::trojan::header::{Command, TrojanRequestHeader};
use crate::protocol::trojan::{new_error, password_to_hash, TrojanUdpStream};
use crate::protocol::{AcceptResult, Address, ProxyAcceptor};
use crate::proxy::relay_tcp;
use async_trait::async_trait;
use serde::Deserialize;
use smol::io::AsyncWriteExt;
use smol::net::TcpStream;
use std::collections::HashSet;
use std::io;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct TrojanAcceptorConfig {
    pub password: String,
    pub fallback_addr: String,
}

pub struct TrojanAcceptor<T: ProxyAcceptor> {
    valid_hash: Arc<HashSet<String>>,
    fallback_addr: Address,
    inner: T,
}

#[async_trait]
impl<T: ProxyAcceptor> ProxyAcceptor for TrojanAcceptor<T> {
    type TS = T::TS;
    type US = TrojanUdpStream<T::TS>;
    async fn accept(&self) -> io::Result<AcceptResult<Self::TS, Self::US>> {
        let (mut stream, addr) = self.inner.accept().await?.unwrap_tcp_with_addr();
        let mut first_packet = Vec::new();
        match TrojanRequestHeader::read_from(&mut stream, &self.valid_hash, &mut first_packet).await
        {
            Ok(header) => {
                log::info!("trojan connection from {}, user = {}", addr, header.hash);
                match header.command {
                    Command::TcpConnect => Ok(AcceptResult::Tcp((stream, header.address))),
                    Command::UdpAssociate => {
                        log::debug!("udp associate");
                        Ok(AcceptResult::Udp(TrojanUdpStream::new(stream)))
                    }
                }
            }
            Err(e) => {
                let fallback_addr = self.fallback_addr.clone();
                log::warn!("fallback to {}", fallback_addr);
                smol::spawn(async move {
                    let inbound = stream;
                    let mut outbound = TcpStream::connect(fallback_addr.to_string()).await.unwrap();
                    let _ = outbound.write_all(&first_packet).await;
                    relay_tcp(inbound, outbound).await;
                })
                .detach();
                Err(new_error(format!("invalid packet: {}", e.to_string())))
            }
        }
    }
}

impl<T: ProxyAcceptor> TrojanAcceptor<T> {
    pub fn new(config: &TrojanAcceptorConfig, inner: T) -> io::Result<Self> {
        let mut valid_hash = HashSet::new();
        let hash = password_to_hash(&config.password);
        valid_hash.insert(hash);
        let fallback_addr = Address::from_str(&config.fallback_addr)?;
        let valid_hash = Arc::new(valid_hash);
        Ok(Self {
            fallback_addr,
            valid_hash,
            inner,
        })
    }
}
