use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

const TOKEN_LENGTH: usize = 64;

pub(crate) fn token_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("WAYTOUCH_IPC_TOKEN_FILE") {
        return Ok(PathBuf::from(path));
    }
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME/XDG_STATE_HOME is not set"))?;
    Ok(base.join("waytouch/ipc-token"))
}

/// Validate the token payload itself: exactly 64 hexadecimal characters,
/// normalised to lowercase for the wire format.
fn validate_token(raw: &str) -> io::Result<String> {
    if raw.len() != TOKEN_LENGTH || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("IPC token must contain exactly {TOKEN_LENGTH} hexadecimal characters"),
        ));
    }
    Ok(raw.to_ascii_lowercase())
}

/// Verify the opened token file descriptor: it must be a regular file that is
/// not accessible by group/other. Checking the descriptor (fstat) instead of a
/// separate `metadata()` call removes the check/read race window, and reading
/// afterwards goes through the same descriptor.
fn verify_token_file(path: &Path, file: &fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("IPC token must be a regular file: {}", path.display()),
            ));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "IPC token file must not be accessible by group/other: {}",
                    path.display()
                ),
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, file);
    }
    Ok(())
}

pub(crate) fn read_token() -> io::Result<String> {
    read_token_from_path(&token_path()?)
}

fn read_token_from_path(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    verify_token_file(path, &file)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    validate_token(contents.trim())
}

pub(crate) fn authenticate(stream: &mut TcpStream) -> io::Result<()> {
    let token = read_token()?;
    authenticate_with_token(stream, &token)
}

pub(crate) fn authenticate_with_token(stream: &mut TcpStream, token: &str) -> io::Result<()> {
    if token.len() != TOKEN_LENGTH || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IPC token must contain exactly 64 hexadecimal characters",
        ));
    }
    stream.write_all(format!("AUTH {}\n", token.to_ascii_lowercase()).as_bytes())?;
    stream.flush()?;

    let mut response = Vec::with_capacity(16);
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        if response.len() >= 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized IPC auth response",
            ));
        }
        if byte[0] != b'\r' {
            response.push(byte[0]);
        }
    }

    if response == b"OK AUTH" {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Android IPC authentication was rejected",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn temp_token_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "waytouch-auth-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn token_validation_accepts_exactly_64_hex_characters_and_normalises_case() {
        let token =
            validate_token("aBcDeF0123456789aBcDeF0123456789aBcDeF0123456789aBcDeF0123456789")
                .unwrap();
        assert_eq!(
            token,
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
        for invalid in [
            "",
            "a".repeat(63).as_str(),
            "a".repeat(65).as_str(),
            &"g".repeat(64),
            &format!("{} ", "a".repeat(64)),
        ] {
            assert!(validate_token(invalid).is_err(), "rejected: {invalid:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_file_mode_is_enforced_on_the_opened_descriptor() {
        use std::os::unix::fs::PermissionsExt;

        let token = "a".repeat(64);
        let cases: &[(u32, bool)] = &[(0o600, true), (0o400, true), (0o640, false), (0o644, false)];
        for (index, (mode, accepted)) in cases.iter().enumerate() {
            let dir = temp_token_dir("mode");
            let path = dir.join("ipc-token");
            fs::write(&path, &token).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(*mode)).unwrap();

            let result = read_token_from_path(&path);
            assert_eq!(
                result.is_ok(),
                *accepted,
                "mode {mode:o} (case {index}) should be accepted={accepted}, got {result:?}"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_file_must_be_a_regular_file() {
        let dir = temp_token_dir("regular");
        let path = dir.join("a-directory");
        fs::create_dir(&path).unwrap();
        let error = read_token_from_path(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn token_file_contents_are_trimmed_and_validated() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_token_dir("contents");
        let path = dir.join("ipc-token");
        fs::write(&path, format!("{}\n", "b".repeat(64))).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let token = read_token_from_path(&path).unwrap();
        assert_eq!(token, "b".repeat(64));

        fs::write(&path, "short".repeat(4)).unwrap();
        assert!(read_token_from_path(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    fn connected_pair() -> (TcpStream, TcpStream) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn handshake_accepts_only_the_ok_auth_response() {
        use std::io::BufRead;

        let token = "c".repeat(64);

        let (mut client, mut server) = connected_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let server = thread::spawn({
            let token = token.clone();
            move || {
                let mut line = String::new();
                std::io::BufReader::new(&mut server)
                    .read_line(&mut line)
                    .unwrap();
                assert_eq!(line.trim(), format!("AUTH {token}"));
                server.write_all(b"OK AUTH\n").unwrap();
                server
            }
        });
        authenticate_with_token(&mut client, &token).unwrap();
        server.join().unwrap();

        let (mut client, mut server) = connected_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let server = thread::spawn(move || {
            let mut line = String::new();
            std::io::BufReader::new(&mut server)
                .read_line(&mut line)
                .unwrap();
            server.write_all(b"NO\n").unwrap();
            server
        });
        let error = authenticate_with_token(&mut client, &token).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        server.join().unwrap();
    }

    #[test]
    fn handshake_rejects_malformed_tokens_before_writing() {
        let (mut client, _server) = connected_pair();
        let error = authenticate_with_token(&mut client, "short").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
