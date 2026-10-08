//! Users and their devices.
//!
//! - `<data>/users.json`: each user's argon2 password hash and library id. Written by
//!   `lightcraft-server user …`, read at every sign-in (so a new user works without a restart).
//! - `<data>/users/<name>/devices.json`: the user's signed-in devices: name, id space, and the
//!   SHA-256 of the device's token (the token itself is never stored). Reloaded when it changes
//!   on disk (`device revoke` while the server runs).
//!
//! Admins (`admin` in `users.json`) also manage the server from its web page (`/admin`, see
//! [`crate::admin`]) with sessions of their own: an admin session is not a device and never
//! syncs; a device token never opens the admin page.
//!
//! A device gets an id space at sign-in ([`crate::MAX_SPACE`] at most): ids it allocates are
//! `space << 32 | n`, so no two devices hand out the same id. Space 0 is never given out: a
//! library that never synced has its ids there, and the first one uploaded keeps them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
    /// May manage the server (users, devices, storage) from `/admin`.
    pub admin: bool,
    /// Photo folders on the server that are this user's library folders, read in place (see
    /// [`crate::folders`]). Only an admin sets them.
    pub folders: Vec<LibraryFolder>,
    /// The server finds the faces in this user's photos and groups them into people (see
    /// [`crate::vision`]). Off until an admin turns it on for them: faces are personal data.
    pub faces: bool,
}

/// One of a user's library folders: a folder on the server, shown to devices by `name`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryFolder {
    /// What devices see as the top folder (`Photos`).
    pub name: String,
    /// The folder on the server (absolute).
    pub path: String,
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

/// An admin session ends after this long without a request.
pub const ADMIN_IDLE: Duration = Duration::from_secs(2 * 3600);

pub fn now() -> u64 {
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
    lightcraft_catalog::safe_file::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    // password and token hashes: for the server's eyes only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
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
    let (admin, folders, faces) = f.users.get(name).map(|u| (u.admin, u.folders.clone(), u.faces)).unwrap_or_default();
    f.users.insert(name.to_string(), User { password: hash_password(password)?, library, admin, folders, faces });
    std::fs::create_dir_all(user_dir(data, name)).map_err(|e| e.to_string())?;
    write_json(&data.join(USERS), &f)
}

/// Let a user manage the server (or not). The last admin can't be taken away while the server
/// has users (there would be no one left to manage it from the web page).
pub fn set_admin(data: &Path, name: &str, on: bool) -> Result<(), String> {
    let mut f = read_users(data)?;
    let others = f.users.iter().any(|(n, u)| n != name && u.admin);
    let u = f.users.get_mut(name).ok_or_else(|| format!("no user `{name}`"))?;
    if !on && u.admin && !others {
        return Err(format!("`{name}` is the only admin: make someone else admin first"));
    }
    u.admin = on;
    write_json(&data.join(USERS), &f)
}

/// Let the server find the faces in a user's photos (or stop: what it found stays until the user
/// deletes it, `DELETE /api/index/faces`).
pub fn set_faces(data: &Path, name: &str, on: bool) -> Result<(), String> {
    let mut f = read_users(data)?;
    f.users.get_mut(name).ok_or_else(|| format!("no user `{name}`"))?.faces = on;
    write_json(&data.join(USERS), &f)
}

/// A library folder's name: one folder name devices show (letters, digits, spaces, `.`, `_`,
/// `-`, `(`, `)`; not starting with `.`; at most 64).
pub fn valid_folder_name(name: &str) -> bool {
    let n = name.trim();
    n == name && !n.is_empty() && n.chars().count() <= 64 && !n.starts_with('.') && n.chars().all(|c| c.is_alphanumeric() || " ._-()".contains(c))
}

/// Give `user` a library folder: `path`, a folder on this server, shown as `name` (default: the
/// folder's own name). The folder must exist and can't hold the server's data, or be inside it.
pub fn add_folder(data: &Path, user: &str, path: &str, name: Option<&str>) -> Result<LibraryFolder, String> {
    let dir = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    if !dir.is_dir() {
        return Err(format!("{path} isn't a folder"));
    }
    let data_dir = std::fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf());
    if dir.starts_with(&data_dir) || data_dir.starts_with(&dir) {
        return Err(format!("{} holds the server's own data ({}): pick a photo folder outside it", dir.display(), data_dir.display()));
    }
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Photos".into()),
    };
    if !valid_folder_name(&name) {
        return Err(format!("`{name}` isn't a usable folder name (letters, digits, spaces, `.`, `_`, `-`, `(`, `)`; at most 64)"));
    }
    let mut f = read_users(data)?;
    let u = f.users.get_mut(user).ok_or_else(|| format!("no user `{user}`"))?;
    if u.folders.iter().any(|x| x.name.eq_ignore_ascii_case(&name)) {
        return Err(format!("{user} has a library folder named `{name}` already (give this one another name)"));
    }
    let path = dir.to_string_lossy().to_string();
    if u.folders.iter().any(|x| Path::new(&x.path).starts_with(&dir) || dir.starts_with(&x.path)) {
        return Err(format!("{path} is inside one of {user}'s library folders, or holds one"));
    }
    let folder = LibraryFolder { name, path };
    u.folders.push(folder.clone());
    write_json(&data.join(USERS), &f)?;
    Ok(folder)
}

/// Stop reading one of `user`'s library folders (its photos stay in the library; their originals
/// can't be downloaded any more).
pub fn remove_folder(data: &Path, user: &str, name: &str) -> Result<(), String> {
    let mut f = read_users(data)?;
    let u = f.users.get_mut(user).ok_or_else(|| format!("no user `{user}`"))?;
    let before = u.folders.len();
    u.folders.retain(|x| !x.name.eq_ignore_ascii_case(name));
    if u.folders.len() == before {
        return Err(format!("{user} has no library folder `{name}`"));
    }
    write_json(&data.join(USERS), &f)
}

/// Is there an admin yet? (Until there is, `/admin` asks for the setup code.)
pub fn has_admin(data: &Path) -> bool {
    read_users(data).is_ok_and(|f| f.users.values().any(|u| u.admin))
}

/// Remove a user's account (their files stay in `users/<name>/` until deleted by hand).
pub fn remove_user(data: &Path, name: &str) -> Result<(), String> {
    let mut f = read_users(data)?;
    let others = f.users.iter().any(|(n, u)| n != name && u.admin);
    match f.users.get(name) {
        None => return Err(format!("no user `{name}`")),
        Some(u) if u.admin && !others => return Err(format!("`{name}` is the only admin: make someone else admin first")),
        Some(_) => {}
    }
    f.users.remove(name);
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
    /// When each device was last heard from (since the server started), unix seconds.
    seen: HashMap<(String, u64), u64>,
    /// Admin sessions by token hash: the admin, and when the session was last used.
    admins: HashMap<String, (String, Instant)>,
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
        let s = self.tokens.get(&token_hash(token)).cloned()?;
        self.seen.insert((s.user.clone(), s.device), now());
        Some(s)
    }

    /// When a device was last heard from since the server started.
    pub fn last_seen(&self, user: &str, device: u64) -> Option<u64> {
        self.seen.get(&(user.to_string(), device)).copied()
    }

    /// Open an admin session (the user must be an admin).
    pub fn admin_login(&mut self, user: &str, password: &str) -> Result<String, LoginError> {
        let users = read_users(&self.data).map_err(LoginError::Failed)?;
        let ok = users.users.get(user).is_some_and(|u| u.admin && password_ok(&u.password, password));
        if !ok {
            return Err(LoginError::Refused);
        }
        let token = random_hex(32).map_err(LoginError::Failed)?;
        self.admins.retain(|_, (_, at)| at.elapsed() < ADMIN_IDLE);
        self.admins.insert(token_hash(&token), (user.to_string(), Instant::now()));
        Ok(token)
    }

    /// The admin an admin session belongs to (still an admin, session not idle too long).
    pub fn check_admin(&mut self, token: &str) -> Option<String> {
        if token.is_empty() || token.len() > 256 {
            return None;
        }
        let key = token_hash(token);
        let (user, at) = self.admins.get_mut(&key)?;
        if at.elapsed() >= ADMIN_IDLE || !read_users(&self.data).is_ok_and(|f| f.users.get(user.as_str()).is_some_and(|u| u.admin)) {
            self.admins.remove(&key);
            return None;
        }
        *at = Instant::now();
        Some(user.clone())
    }

    pub fn admin_logout(&mut self, token: &str) {
        self.admins.remove(&token_hash(token));
    }

    /// Forget a removed user's devices.
    pub fn forget(&mut self, user: &str) {
        self.loaded.remove(user);
        self.tokens.retain(|_, s| s.user != user);
        self.admins.retain(|_, (u, _)| u != user);
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
        // (the last one is the server's own: photos from library folders)
        if space >= MAX_SPACE {
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
        self.seen.insert((user.to_string(), id), d.created);
        Ok((token, d, u.library.clone()))
    }

    /// Sign a device out by id (the admin page).
    pub fn revoke(&mut self, user: &str, device: u64) -> Result<(), String> {
        revoke(&self.data, user, device)?;
        self.loaded.remove(user);
        self.reload(user);
        Ok(())
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(data.join(USERS)).unwrap().permissions().mode() & 0o777, 0o600);
        }
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
