use super::util::config_dir;
use anyhow::{anyhow, Context};
use clap::arg_enum;
use dialoguer::{Input, Password};
use log::{debug, error, info, warn};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::str::FromStr;

arg_enum! {
    #[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub enum VpnProvider {
    PrivateInternetAccess,
    Mullvad,
    TigerVpn,
    Custom,
}
}

impl VpnProvider {
    pub fn alias(&self) -> String {
        match self {
            Self::PrivateInternetAccess => String::from("pia"),
            Self::Mullvad => String::from("mv"),
            Self::TigerVpn => String::from("tig"),
            Self::Custom => String::from("cus"),
        }
    }

    pub fn dns(&self) -> anyhow::Result<Vec<IpAddr>> {
        let res = match self {
            Self::PrivateInternetAccess => vec![
                IpAddr::from_str("209.222.18.222"),
                IpAddr::from_str("209.222.18.218"),
            ],
            Self::Mullvad => vec![IpAddr::from_str("193.138.218.74")],
            Self::TigerVpn => vec![IpAddr::from_str("8.8.8.8"), IpAddr::from_str("8.8.4.4")],
            Self::Custom => vec![IpAddr::from_str("8.8.8.8"), IpAddr::from_str("8.8.4.4")],
        };

        Ok(res
            .into_iter()
            .collect::<Result<Vec<IpAddr>, std::net::AddrParseError>>()?)
    }
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub enum OpenVpnProtocol {
    UDP,
    TCP,
}

impl FromStr for OpenVpnProtocol {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "udp" => Ok(Self::UDP),
            "tcp-client" => Ok(Self::TCP),
            "tcp" => Ok(Self::TCP),
            _ => Err(anyhow!("Unknown VPN protocol: {}", s)),
        }
    }
}

impl Display for OpenVpnProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let out = match self {
            Self::UDP => "udp",
            Self::TCP => "tcp",
        };
        write!(f, "{}", out)
    }
}

arg_enum! {
    #[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub enum Protocol {
    OpenVpn,
    Wireguard,
}
}

// pub enum Firewall {
//     IpTables,
//     NfTables,
//     Ufw,
// }

#[derive(Serialize, Deserialize)]
pub struct VpnServer {
    pub name: String,
    pub alias: String,
    pub host: String,
    pub port: Option<u16>,
    pub protocol: Option<OpenVpnProtocol>,
}

pub fn get_serverlist(provider: &VpnProvider) -> anyhow::Result<Vec<VpnServer>> {
    let mut list_path = config_dir()?;
    list_path.push(format!(
        "vopono/{}/openvpn/serverlist.csv",
        provider.alias()
    ));
    let file = File::open(&list_path).with_context(|| {
        format!(
            "Could not get serverlist for provider: {}, path: {}",
            provider.to_string(),
            list_path.to_string_lossy()
        )
    })?;
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(file);
    let mut resultvec = Vec::new();

    for row in rdr.deserialize() {
        resultvec.push(row?);
    }
    Ok(resultvec)
}

// OpenVPN
pub fn find_host_from_alias(
    alias: &str,
    serverlist: &[VpnServer],
) -> anyhow::Result<(String, u16, String, OpenVpnProtocol)> {
    let alias = alias.to_lowercase();
    let record = serverlist
        .iter()
        .filter(|x| {
            x.name.starts_with(&alias)
                || x.alias.starts_with(&alias)
                || x.name.replace("_", "-").starts_with(&alias)
        })
        .collect::<Vec<&VpnServer>>();

    if record.is_empty() {
        Err(anyhow!(
            "Could not find server alias {} in serverlist",
            &alias
        ))
    } else {
        let record = record
            .choose(&mut rand::thread_rng())
            .expect("Could not find server alias");

        let port = if record.port.is_none() {
            warn!(
                "Using default OpenVPN port 1194 for {}, as no port provided",
                &record.host
            );
            1194
        } else {
            record.port.unwrap()
        };

        let protocol = if record.protocol.is_none() {
            warn!(
                "Using UDP as default OpenVPN protocol for {}, as no protocol provided",
                &record.host
            );
            OpenVpnProtocol::UDP
        } else {
            record.protocol.clone().unwrap()
        };
        info!("Chosen server: {}:{} {}", record.host, port, protocol);
        Ok((record.host.clone(), port, record.alias.clone(), protocol))
    }
}

// TODO: Can we avoid storing plaintext passwords?
// TODO: Allow not storing credentials
// OpenVPN only
pub fn get_auth(provider: &VpnProvider) -> anyhow::Result<()> {
    let mut auth_path = config_dir()?;
    auth_path.push(format!("vopono/{}/openvpn/auth.txt", provider.alias()));
    let file = File::open(&auth_path);
    match file {
        Ok(f) => {
            debug!("Read auth file: {}", auth_path.to_string_lossy());
            let bufreader = BufReader::new(f);
            let mut iter = bufreader.lines();
            let _username = iter.next().with_context(|| "No username")??;
            let _password = iter.next().with_context(|| "No password")??;
            Ok(())
        }
        Err(_) => {
            debug!(
                "No auth file: {} - prompting user",
                auth_path.to_string_lossy()
            );

            let user_prompt = match provider {
                VpnProvider::Mullvad => "Mullvad account number",
                VpnProvider::TigerVpn => {
                    "OpenVPN username (see https://www.tigervpn.com/dashboard/geeks )"
                }
                VpnProvider::PrivateInternetAccess => "PrivateInternetAccess username",
                VpnProvider::Custom => "OpenVPN username",
            };
            let mut username = Input::<String>::new().with_prompt(user_prompt).interact()?;
            if *provider == VpnProvider::Mullvad {
                username.retain(|c| !c.is_whitespace() && c.is_digit(10));
                if username.len() != 16 {
                    return Err(anyhow!(
                        "Mullvad account number should be 16 digits!, parsed: {}",
                        username
                    ));
                }
            }

            let password = if *provider == VpnProvider::Mullvad {
                String::from("m")
            } else {
                Password::new()
                    .with_prompt("Password")
                    .with_confirmation("Confirm password", "Passwords did not match")
                    .interact()?
            };

            let mut writefile = File::create(&auth_path)
                .with_context(|| format!("Could not create auth file: {}", auth_path.display()))?;
            write!(writefile, "{}\n{}\n", username, password)?;
            info!("Credentials written to: {}", auth_path.to_string_lossy());
            Ok(())
        }
    }
}

// TODO: For providers that provide both, check if system has wireguard capability first for
// default
pub fn get_protocol(
    provider: &VpnProvider,
    protocol: Option<Protocol>,
) -> anyhow::Result<Protocol> {
    match protocol {
        Some(Protocol::Wireguard) => match provider {
            VpnProvider::Mullvad => Ok(Protocol::Wireguard),
            VpnProvider::TigerVpn => {
                error!("Wireguard not implemented for TigerVPN");
                Err(anyhow!("Wireguard not implemented for TigerVPN"))
            }
            VpnProvider::PrivateInternetAccess => {
                error!("Wireguard not implemented for PrivateInternetAccess");
                Err(anyhow!(
                    "Wireguard not implemented for PrivateInternetAccess"
                ))
            }
            VpnProvider::Custom => Ok(Protocol::Wireguard),
        },
        Some(Protocol::OpenVpn) => Ok(Protocol::OpenVpn),
        None => match provider {
            VpnProvider::Mullvad => Ok(Protocol::Wireguard),
            VpnProvider::TigerVpn => Ok(Protocol::OpenVpn),
            VpnProvider::PrivateInternetAccess => Ok(Protocol::OpenVpn),
            VpnProvider::Custom => Ok(Protocol::Wireguard),
        },
    }
}
