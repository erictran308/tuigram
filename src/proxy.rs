//! A proxy for the connection to Telegram, where it's blocked: SOCKS5,
//! HTTP, or one of Telegram's MTProto proxies. It's always given as a
//! link: `proxy` in `settings.toml`, `TG_PROXY`, `:proxy`, or a proxy link
//! in a message, which asks first. TDLib remembers proxies too, but the
//! setting decides: without one, tuigram connects directly.

use tdlib_rs::enums::ProxyType;
use tdlib_rs::types::{Proxy, ProxyTypeHttp, ProxyTypeMtproto, ProxyTypeSocks5};

/// Hosts that serve Telegram's own links.
const TELEGRAM_HOSTS: [&str; 3] = ["t.me", "telegram.me", "telegram.dog"];
/// Longest host name DNS allows.
const MAX_HOST: usize = 253;
/// More than any MTProto secret, or a user name or password, needs.
const MAX_SECRET: usize = 512;

/// What a link says to use, or why it can't be used.
pub fn parse(link: &str) -> Result<Proxy, String> {
    let link = link.trim();
    let lower = link.to_ascii_lowercase();
    if let Some(rest) = strip(link, &lower, "socks5://") {
        let (user, pass, server, port) = authority(rest)?;
        return Ok(Proxy {
            server,
            port,
            r#type: ProxyType::Socks5(ProxyTypeSocks5 {
                username: user,
                password: pass,
            }),
        });
    }
    if let Some(rest) = strip(link, &lower, "http://") {
        let (user, pass, server, port) = authority(rest)?;
        return Ok(Proxy {
            server,
            port,
            r#type: ProxyType::Http(ProxyTypeHttp {
                username: user,
                password: pass,
                http_only: false,
            }),
        });
    }
    let (kind, query) = telegram_link(link, &lower).ok_or(NOT_A_PROXY)?;
    let get = |key: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| decode(v))
            .transpose()
            .map(Option::unwrap_or_default)
    };
    let server = host(&get("server")?)?;
    let port = port(&get("port")?)?;
    let r#type = match kind {
        Kind::Mtproto => {
            let secret = get("secret")?;
            if secret.is_empty() {
                return Err("The link has no secret".into());
            }
            ProxyType::Mtproto(ProxyTypeMtproto {
                secret: secret_text(secret)?,
            })
        }
        Kind::Socks5 => ProxyType::Socks5(ProxyTypeSocks5 {
            username: secret_text(get("user")?)?,
            password: secret_text(get("pass")?)?,
        }),
    };
    Ok(Proxy {
        server,
        port,
        r#type,
    })
}

/// A link in a message that's one of Telegram's proxy links, which tuigram
/// offers to use, rather than open in the browser.
pub fn is_link(url: &str) -> bool {
    telegram_link(url, &url.to_ascii_lowercase()).is_some()
}

/// The proxy in words, without its password or secret, e.g.
/// "SOCKS5 proxy 10.0.0.1:1080".
pub fn describe(proxy: &Proxy) -> String {
    format!("{} proxy {}", kind(proxy), address(proxy))
}

/// What kind of proxy it is: "SOCKS5", "HTTP" or "MTProto".
pub fn kind(proxy: &Proxy) -> &'static str {
    match proxy.r#type {
        ProxyType::Socks5(_) => "SOCKS5",
        ProxyType::Http(_) => "HTTP",
        ProxyType::Mtproto(_) => "MTProto",
    }
}

/// Its server and port, `host:port`, or `[address]:port` for IPv6.
pub fn address(proxy: &Proxy) -> String {
    if proxy.server.contains(':') {
        format!("[{}]:{}", proxy.server, proxy.port)
    } else {
        format!("{}:{}", proxy.server, proxy.port)
    }
}

const NOT_A_PROXY: &str =
    "Not a proxy link: give socks5://host:port, http://host:port or a t.me/proxy link";

enum Kind {
    Mtproto,
    Socks5,
}

/// `rest` after `prefix`, which `lower` (the link in lowercase) starts with.
fn strip<'a>(link: &'a str, lower: &str, prefix: &str) -> Option<&'a str> {
    lower.starts_with(prefix).then(|| &link[prefix.len()..])
}

/// Telegram's proxy links: `https://t.me/proxy?…`, `t.me/socks?…`,
/// `tg://proxy?…`; the kind, and what's after the `?`.
fn telegram_link<'a>(link: &'a str, lower: &str) -> Option<(Kind, &'a str)> {
    let (path, query) = link.split_once('?')?;
    let path = &lower[..path.len()];
    let path = path.trim_end_matches('/');
    let name = match path.strip_prefix("tg://") {
        Some(name) => name,
        None => {
            let rest = path
                .strip_prefix("https://")
                .or_else(|| path.strip_prefix("http://"))
                .unwrap_or(path);
            let (host, name) = rest.split_once('/')?;
            let host = host.strip_prefix("www.").unwrap_or(host);
            if !TELEGRAM_HOSTS.contains(&host) {
                return None;
            }
            name
        }
    };
    let kind = match name {
        "proxy" => Kind::Mtproto,
        "socks" => Kind::Socks5,
        _ => return None,
    };
    // A fragment isn't part of the query.
    let query = query.split('#').next().unwrap_or_default();
    Some((kind, query))
}

/// `user:pass@host:port`, the user and password optional.
fn authority(rest: &str) -> Result<(String, String, String, i32), String> {
    let rest = rest.trim_end_matches('/');
    if rest.contains(['/', '?', '#']) {
        return Err(NOT_A_PROXY.into());
    }
    let (login, place) = match rest.rsplit_once('@') {
        Some((login, place)) => (Some(login), place),
        None => (None, rest),
    };
    let (user, pass) = match login {
        Some(login) => {
            let (user, pass) = login.split_once(':').unwrap_or((login, ""));
            (secret_text(decode(user)?)?, secret_text(decode(pass)?)?)
        }
        None => (String::new(), String::new()),
    };
    let (server, port_text) = match place.strip_prefix('[') {
        // An IPv6 address, in brackets.
        Some(v6) => {
            let (server, after) = v6
                .split_once(']')
                .ok_or("A [ in the address isn't closed")?;
            (server, after.strip_prefix(':').unwrap_or_default())
        }
        None => place.rsplit_once(':').unwrap_or((place, "")),
    };
    Ok((user, pass, host(server)?, port(port_text)?))
}

/// A host name or address: only the characters they're made of, so
/// nothing odd can be shown, or sent to TDLib.
fn host(text: &str) -> Result<String, String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':');
    if text.is_empty() {
        return Err("The link has no server".into());
    }
    if text.len() > MAX_HOST || !text.chars().all(ok) {
        return Err("The server isn't a host name or an address".into());
    }
    Ok(text.to_string())
}

fn port(text: &str) -> Result<i32, String> {
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port.into()),
        _ if text.is_empty() => Err("The link has no port, as in host:1080".into()),
        _ => Err("The port must be a number from 1 to 65535".into()),
    }
}

/// A user name, password or MTProto secret: no control characters, and
/// not too long.
fn secret_text(text: String) -> Result<String, String> {
    if text.len() > MAX_SECRET || text.chars().any(char::is_control) {
        return Err("The link's user name, password or secret can't be used".into());
    }
    Ok(text)
}

/// Undoes `%XX` escapes, and `+` for a space as forms write it.
fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or("A % in the link isn't followed by two hex digits")?;
                out.push(hex);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "The link isn't valid text".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socks(username: &str, password: &str) -> ProxyType {
        ProxyType::Socks5(ProxyTypeSocks5 {
            username: username.into(),
            password: password.into(),
        })
    }

    #[test]
    fn socks5_and_http_links_say_where_and_who() {
        let proxy = parse("socks5://10.0.0.1:1080").unwrap();
        assert_eq!(proxy.server, "10.0.0.1");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.r#type, socks("", ""));
        assert_eq!(describe(&proxy), "SOCKS5 proxy 10.0.0.1:1080");

        let proxy = parse(" SOCKS5://me:p%40ss@proxy.example.com:9050/ ").unwrap();
        assert_eq!(proxy.server, "proxy.example.com");
        assert_eq!(proxy.r#type, socks("me", "p@ss"));

        let proxy = parse("http://[2001:db8::1]:3128").unwrap();
        assert_eq!(proxy.server, "2001:db8::1");
        assert!(matches!(proxy.r#type, ProxyType::Http(_)));
        assert_eq!(describe(&proxy), "HTTP proxy [2001:db8::1]:3128");
    }

    #[test]
    fn telegrams_proxy_links_are_read_wherever_they_come_from() {
        for link in [
            "https://t.me/proxy?server=1.2.3.4&port=443&secret=ee0123abcd",
            "t.me/proxy?port=443&secret=ee0123abcd&server=1.2.3.4",
            "tg://proxy?server=1.2.3.4&port=443&secret=ee0123abcd",
            "https://www.telegram.me/proxy/?server=1.2.3.4&port=443&secret=ee0123abcd#x",
        ] {
            let proxy = parse(link).unwrap();
            assert_eq!(proxy.server, "1.2.3.4", "{link}");
            assert_eq!(proxy.port, 443);
            let ProxyType::Mtproto(m) = &proxy.r#type else {
                panic!("{link}");
            };
            assert_eq!(m.secret, "ee0123abcd");
            assert!(is_link(link));
        }
        let proxy = parse("https://t.me/socks?server=h.example&port=1080&user=a&pass=b").unwrap();
        assert_eq!(proxy.r#type, socks("a", "b"));
        assert!(!is_link("https://t.me/durov"));
        assert!(!is_link("https://t.me.evil.example/proxy?server=1.2.3.4"));
    }

    #[test]
    fn a_link_that_cant_be_used_says_why() {
        let why = |link| parse(link).unwrap_err();
        assert!(why("https://example.com").starts_with("Not a proxy link"));
        assert!(why("ftp://host:21").starts_with("Not a proxy link"));
        assert!(why("socks5://host").contains("no port"));
        assert!(why("socks5://host:99999").contains("1 to 65535"));
        assert!(why("socks5://host:0").contains("1 to 65535"));
        assert!(why("socks5://ho\u{1b}st:1080").contains("isn't a host name"));
        assert!(why("socks5://host:1080/path").starts_with("Not a proxy link"));
        assert!(why("tg://proxy?server=1.2.3.4&port=443").contains("no secret"));
        assert!(why("tg://proxy?port=443&secret=ee").contains("no server"));
        assert!(why("socks5://a%0Ab@host:1080").contains("can't be used"));
        assert!(why("socks5://a%zzb@host:1080").contains("two hex digits"));
    }
}
