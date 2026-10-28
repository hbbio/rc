//! OpenSSH authentication and host verification are retained across both SSH
//! transports. RC does not collect or persist private keys or passwords.
use std::io;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    host: String,
    user: Option<String>,
    port: Option<u16>,
}

impl Endpoint {
    pub fn parse(input: &str) -> io::Result<Self> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Use [user@]host[:port], with IPv6 hosts in brackets; enter the directory separately",
            )
        };
        if input.is_empty()
            || input.len() > 1024
            || input
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(invalid());
        }
        let (user, address) = match input.split_once('@') {
            Some((user, address)) => {
                if user.is_empty()
                    || user.starts_with('-')
                    || !user
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
                {
                    return Err(invalid());
                }
                (Some(user.to_string()), address)
            }
            None => (None, input),
        };
        let (host, port) = if let Some(ipv6) = address.strip_prefix('[') {
            let (host, tail) = ipv6.split_once(']').ok_or_else(invalid)?;
            let (ip, zone) = host
                .split_once('%')
                .map_or((host, None), |(ip, zone)| (ip, Some(zone)));
            ip.parse::<std::net::Ipv6Addr>().map_err(|_| invalid())?;
            if zone.is_some_and(|zone| {
                zone.is_empty()
                    || !zone
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            }) {
                return Err(invalid());
            }
            (
                host,
                if tail.is_empty() {
                    None
                } else {
                    Some(tail.strip_prefix(':').ok_or_else(invalid)?)
                },
            )
        } else {
            let (host, port) = address
                .split_once(':')
                .map_or((address, None), |(host, port)| (host, Some(port)));
            if host.is_empty()
                || host.starts_with('-')
                || !host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            {
                return Err(invalid());
            }
            (host, port)
        };
        let port = port
            .map(|port| {
                port.parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(invalid)
            })
            .transpose()?;
        Ok(Self {
            host: host.to_string(),
            user,
            port,
        })
    }
    pub fn authority(&self) -> String {
        let mut result = self
            .user
            .as_ref()
            .map_or_else(String::new, |user| format!("{user}@"));
        if self.host.contains(':') {
            result.push_str(&format!("[{}]", self.host));
        } else {
            result.push_str(&self.host);
        }
        if let Some(port) = self.port {
            result.push_str(&format!(":{port}"));
        }
        result
    }
    pub(super) fn command(&self, program: &Path, subsystem: bool) -> Command {
        let mut command = Command::new(program);
        command.args(["-T", "-a", "-x"]);
        // CLI values take precedence over configuration. In particular, do not
        // reuse a master connection whose host verification policy is unknown.
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ConnectTimeout=15",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=2",
            "ControlPath=none",
            "ControlMaster=no",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "RemoteCommand=none",
            "RequestTTY=no",
            "ForkAfterAuthentication=no",
            "StdinNull=no",
        ] {
            command.arg("-o").arg(option);
        }
        if let Some(user) = &self.user {
            command.arg("-l").arg(user);
        }
        if let Some(port) = self.port {
            command.arg("-p").arg(port.to_string());
        }
        if subsystem {
            command.arg("-s");
        }
        command.arg("--").arg(&self.host);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_literal_arguments_and_enforce_host_verification() {
        for input in [
            "-oProxyCommand=bad",
            "user;id@host",
            "host/path",
            "user:secret@host",
            "host\nother",
            "user@host@other",
            "host:0",
            "host:65536",
            "[bad]",
            "host$(id)",
        ] {
            assert!(Endpoint::parse(input).is_err(), "{input}");
        }
        for address in ["alias", "user@host:22", "user@[::1]:2222", "[fe80::1%en0]"] {
            let endpoint = Endpoint::parse(address).unwrap();
            assert_eq!(endpoint.authority(), address);
            let command = endpoint.command(Path::new("ssh"), true);
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            for option in [
                "StrictHostKeyChecking=yes",
                "BatchMode=yes",
                "ControlPath=none",
                "PermitLocalCommand=no",
                "ClearAllForwardings=yes",
            ] {
                assert!(args.windows(2).any(|pair| pair == ["-o", option]));
            }
            assert_eq!(
                &args[args.len() - 3..],
                &["-s", "--", endpoint.host.as_str()]
            );
        }
    }
}
