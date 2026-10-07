//! Users and their devices.
//!
//! - `<data>/users.json`: each user's argon2 password hash and library id. Written by
//!   `lightcraft-server user …`, read at every sign-in (so a new user works without a restart).
//! - `<data>/users/<name>/devices.json`: the user's signed-in devices: name, id space, and the
//!   SHA-256 of the device's token (the token itself is never stored). Reloaded when it changes
//!   on disk (`device revoke` while the server runs).
//!
//! A device gets an id space at sign-in ([`crate::MAX_SPACE`] at most): ids it allocates are
//! `space << 32 | n`, so no two devices hand out the same id. Space 0 is never given out: a
//! library that never synced has its ids there, and the first one uploaded keeps them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::MAX_SPACE;

pub const USERS: &str = "users.json";
pub const DEVICES: &str = "devices.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    /// argon2id PHC string.
    pub password: String,
    /// Identifies this user's library (a device only resumes the library it is a copy of).
    pub library: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UsersFile {
    pub users: BTreeMap<String, User>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Device {
    pub id: u64,
    pub name: String,
    pub space: u32,
    /// SHA-256 of the token, hex.
    pub token: String,
    /// Unix seconds.
    pub created: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DevicesFile {
    pub next_space: u32,
    pub next_id: u64,
    pub devices: Vec<Device>,
}

/// A user name that is safe as a folder name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with('.') && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub fn user_dir(data: &Path, name: &str) -> PathBuf {
    data.join("users").join(name)
}

fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `n` random bytes as hex.
pub fn random_hex(n: usize) -> Result<String, String> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| format!("no random numbers: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

pub fn token_hash(token: &str) -> String {
    sha2::Sha256::digest(token.as_bytes()).iter().map(|x| format!("{x:02x}")).collect()
}

fn read_json<T: Default + serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{} is damaged: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    lightcraft_catalog::safe_file::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_users(data: &Path) -> Result<UsersFile, String> {
    read_json(&data.join(USERS))
}

fn hash_password(password: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| format!("no random numbers: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    argon2::Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

fn password_ok(stored: &str, password: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|h| argon2::Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// Add a user (or, with `replace`, set an existing user's password).
pub fn set_user(data: &Path, name: &str, password: &str, replace: bool) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("`{name}` isn't a usable user name (letters, digits, `.`, `_`, `-`; at most 64)"));
    }
    if password.chars().count() < 8 {
        return Err("the password needs at least 8 characters".into());
    }
    let mut f = read_users(data)?;
    let library = match f.users.get(name) {
        Some(_) if !replace => return Err(format!("user `{name}` exists (use `user passwd` to change the password)")),
        Some(u) => u.library.clone(),
        None if replace => return Err(format!("no user `{name}`")),
        None => random_hex(16)?,
    };
    f.users.insert(name.to_string(), User { password: hash_password(password)?, library });
    std::fs::create_dir_all(user_dir(data, name)).map_err(|e| e.to_string())?;
    write_json(&data.join(USERS), &f)
}

/// Remove a user's account (their files stay in `users/<name>/` until deleted by hand).
pub fn remove_user(data: &Path, name: &str) -> Result<(), String> {
    let mut f = read_users(data)?;
    if f.users.remove(name).is_none() {
        return Err(format!("no user `{name}`"));
    }
    write_json(&data.join(USERS), &f)?;
    // signed-in devices stop working at once
    write_json(&user_dir(data, name).join(DEVICES), &DevicesFile::default())
}

pub fn devices(data: &Path, name: &str) -> Result<DevicesFile, String> {
    read_json(&user_dir(data, name).join(DEVICES))
}

/// Sign a device out (its token stops working).
pub fn revoke(data: &Path, name: &str, device: u64) -> Result<(), String> {
    let mut f = devices(data, name)?;
    let before = f.devices.len();
    f.devices.retain(|d| d.id != device);
    if f.devices.len() == before {
        return Err(format!("`{name}` has no device {device}"));
    }
    write_json(&user_dir(data, name).join(DEVICES), &f)
}

/// What a signed-in device is.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub user: String,
    pub device: u64,
}

/// The signed-in devices, by token hash (kept in step with the devices files).
#[derive(Default)]
pub struct Accounts {
    data: PathBuf,
    /// Per user: their devices file's time and size when it was read, and its devices.
    loaded: HashMap<String, (Option<(SystemTime, u64)>, Vec<Device>)>,
    tokens: HashMap<String, Session>,
}

pub enum LoginError {
    Refused,
    Failed(String),
}

impl Accounts {
    pub fn new(data: &Path) -> Accounts {
        let mut a = Accounts { data: data.to_path_buf(), ..Default::default() };
        if let Ok(f) = read_users(data) {
            for name in f.users.keys() {
                a.reload(name);
            }
        }
        a
    }

    fn devices_path(&self, user: &str) -> PathBuf {
        user_dir(&self.data, user).join(DEVICES)
    }

    /// Re-read a user's devices file when it changed on disk.
    fn reload(&mut self, user: &str) {
        let path = self.devices_path(user);
        let mtime = std::fs::metadata(&path).and_then(|m| Ok((m.modified()?, m.len()))).ok();
        if self.loaded.get(user).is_some_and(|(t, _)| *t == mtime && mtime.is_some()) {
            return;
        }
        let devices = match read_json::<DevicesFile>(&path) {
            Ok(f) => f.devices,
            Err(e) => {
                log::error!("{e}");
                Vec::new()
            }
        };
        self.tokens.retain(|_, s| s.user != user);
        for d in &devices {
            self.tokens.insert(d.token.clone(), Session { user: user.to_string(), device: d.id });
        }
        self.loaded.insert(user.to_string(), (mtime, devices));
    }

    /// The device a bearer token belongs to.
    pub fn check(&mut self, token: &str) -> Option<Session> {
        if token.is_empty() || token.len() > 256 {
            return None;
        }
        let s = self.tokens.get(&token_hash(token))?.clone();
        // revoked on disk meanwhile?
        self.reload(&s.user);
        self.tokens.get(&token_hash(token)).cloned()
    }

    /// Sign a device in: a new token and id space.
    pub fn login(&mut self, user: &str, password: &str, device_name: &str) -> Result<(String, Device, String), LoginError> {
        let users = read_users(&self.data).map_err(LoginError::Failed)?;
        let Some(u) = users.users.get(user).filter(|_| valid_name(user)) else {
            // the same work as a real check, so timing doesn't tell which users exist
            static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let _ = password_ok(DUMMY.get_or_init(|| hash_password("not a password").unwrap_or_default()), password);
            return Err(LoginError::Refused);
        };
        if !password_ok(&u.password, password) {
            return Err(LoginError::Refused);
        }
        let path = self.devices_path(user);
        let mut f: DevicesFile = read_json(&path).map_err(LoginError::Failed)?;
        let space = f.next_space.max(1);
        if space > MAX_SPACE {
            return Err(LoginError::Failed("this user has signed in too many devices".into()));
        }
        let token = random_hex(32).map_err(LoginError::Failed)?;
        let id = f.next_id.max(1);
        let name: String = device_name.chars().filter(|c| !c.is_control()).take(100).collect();
        let d = Device { id, name, space, token: token_hash(&token), created: now() };
        f.next_space = space + 1;
        f.next_id = id + 1;
        f.devices.push(d.clone());
        write_json(&path, &f).map_err(LoginError::Failed)?;
        self.loaded.remove(user);
        self.reload(user);
        Ok((token, d, u.library.clone()))
    }

    /// Sign the device with this token out.
    pub fn logout(&mut self, s: &Session) -> Result<(), String> {
        revoke(&self.data, &s.user, s.device)?;
        self.loaded.remove(&s.user);
        self.reload(&s.user);
        Ok(())
    }

    pub fn library_of(&self, user: &str) -> Result<String, String> {
        read_users(&self.data)?.users.get(user).map(|u| u.library.clone()).ok_or_else(|| format!("no user `{user}`"))
    }

    pub fn devices_of(&mut self, user: &str) -> Vec<Device> {
        self.reload(user);
        self.loaded.get(user).map(|(_, d)| d.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-server-acc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn users_devices_and_tokens() {
        let data = temp("users");
        assert!(set_user(&data, "../x", "longenough", false).is_err());
        assert!(set_user(&data, "ann", "short", false).is_err());
        set_user(&data, "ann", "correct horse", false).unwrap();
        assert!(set_user(&data, "ann", "correct horse", false).is_err(), "no silent overwrite");
        let mut a = Accounts::new(&data);
        assert!(matches!(a.login("ann", "wrong", "Mac"), Err(LoginError::Refused)));
        assert!(matches!(a.login("bob", "correct horse", "Mac"), Err(LoginError::Refused)));
        let (t1, d1, lib) = a.login("ann", "correct horse", "Mac").map_err(|_| ()).unwrap();
        let (t2, d2, _) = a.login("ann", "correct horse", "iPad").map_err(|_| ()).unwrap();
        assert_eq!((d1.space, d2.space), (1, 2), "space 0 is never handed out");
        assert_eq!(lib.len(), 32);
        assert_eq!(a.check(&t1), Some(Session { user: "ann".into(), device: d1.id }));
        assert!(!std::fs::read_to_string(user_dir(&data, "ann").join(DEVICES)).unwrap().contains(&t1), "tokens are stored hashed");
        // revoked from the command line while the server runs
        std::thread::sleep(std::time::Duration::from_millis(20));
        revoke(&data, "ann", d1.id).unwrap();
        assert_eq!(a.check(&t1), None);
        assert!(a.check(&t2).is_some());
        // a new password keeps the library; removing the user signs every device out
        set_user(&data, "ann", "battery staple", true).unwrap();
        assert_eq!(a.library_of("ann").unwrap(), lib);
        std::thread::sleep(std::time::Duration::from_millis(20));
        remove_user(&data, "ann").unwrap();
        assert_eq!(a.check(&t2), None);
        assert_eq!(a.check(""), None);
        let _ = std::fs::remove_dir_all(&data);
    }
}
