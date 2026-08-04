//! First run: getting someone from "I installed Jotbay" to "my notes sync".
//!
//! A native installer cannot clone a private repository, so the app has to ask
//! where the notes live. The cheapest thing to build would be a folder picker,
//! but that assumes the person already made a repo and knows what a remote is —
//! which is exactly the assumption that makes "clone this and run a shell
//! script" unusable as onboarding for anyone but its author.
//!
//! So the offered path is guided: `gh` already knows who you are on any machine
//! that could install this, so Jotbay asks it to create the private repository
//! and clones the result. Pasting a URL and adopting an existing folder stay
//! available for people who have their own arrangements.
//!
//! A vault created here holds notes and nothing else — no toolchain, no source.
//! The program arrives from the installer; the repository is just the notes.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What the machine can offer, so a first-run screen can grey out what will
/// not work rather than failing after the user commits to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupCapabilities {
    pub git: bool,
    pub gh: bool,
    pub gh_authenticated: bool,
    /// The GitHub account `gh` is signed in as, when it is.
    pub login: Option<String>,
    pub default_location: String,
    /// Whether the desktop app can be found, so an offer to put a shortcut to
    /// it somewhere is not made when there is nothing to point at.
    pub app_installed: bool,
    /// Where a shortcut would go — the user's desktop, XDG setting honoured.
    pub desktop: String,
}

pub fn capabilities() -> SetupCapabilities {
    let git = has("git");
    let gh = has("gh");
    let gh_authenticated = gh
        && crate::proc::quiet("gh")
            .args(["auth", "status"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

    let login = if gh_authenticated {
        crate::proc::quiet("gh")
            .args(["api", "user", "--jq", ".login"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    SetupCapabilities {
        git,
        gh,
        gh_authenticated,
        login,
        default_location: crate::default_root().to_string_lossy().to_string(),
        app_installed: crate::shortcut::locate_app().is_some(),
        desktop: crate::shortcut::default_location()
            .to_string_lossy()
            .to_string(),
    }
}

fn has(cmd: &str) -> bool {
    crate::proc::quiet(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run(cmd: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut c = crate::proc::quiet(cmd);
    c.args(args);
    c.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    let out = c.output().map_err(|e| Error::Other(format!("{cmd}: {e}")))?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Create a private repository and clone it, ready to sync.
pub fn create_and_clone(name: &str, destination: &Path) -> Result<PathBuf> {
    let caps = capabilities();
    if !caps.git {
        return Err(Error::Other("git is not installed".into()));
    }
    if !caps.gh_authenticated {
        return Err(Error::Other(
            "gh is not signed in — run `gh auth login`, then try again".into(),
        ));
    }
    guard_destination(destination)?;

    let owner = caps
        .login
        .ok_or_else(|| Error::Other("could not read the GitHub account name".into()))?;
    let slug = format!("{owner}/{name}");

    // Check before creating. Otherwise the failure surfaces as raw GraphQL —
    // "Name already exists on this account (createRepository)" — at the worst
    // possible moment, when someone is three clicks into first-run setup.
    let exists = crate::proc::quiet("gh")
        .args(["repo", "view", &slug, "--json", "name"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        return Err(Error::Other(format!(
            "you already have a repository called {name}.              Choose another name, or use it as-is with `--clone {slug}`."
        )));
    }

    // --clone would put it in the current directory; clone explicitly so the
    // chosen location is honoured.
    run("gh", &["repo", "create", &slug, "--private"], None)?;
    run(
        "gh",
        &["repo", "clone", &slug, &destination.to_string_lossy()],
        None,
    )?;

    seed(destination)?;
    publish_initial(destination)?;
    Ok(destination.to_path_buf())
}

/// Clone a repository the user already has.
pub fn clone_existing(url: &str, destination: &Path) -> Result<PathBuf> {
    guard_destination(destination)?;
    run("git", &["clone", url, &destination.to_string_lossy()], None)?;
    // An existing vault is left exactly as it is. Seeding only fills gaps.
    seed(destination)?;
    publish_initial(destination)?;
    Ok(destination.to_path_buf())
}

/// Adopt a directory that is already a git clone.
pub fn adopt(path: &Path) -> Result<PathBuf> {
    if !path.join(".git").exists() {
        return Err(Error::Other(format!(
            "{} is not a git repository",
            path.display()
        )));
    }
    seed(path)?;
    publish_initial(path)?;
    Ok(path.to_path_buf())
}

/// Give the branch a first commit and an upstream, if it has neither.
///
/// `gh repo create` makes an genuinely empty repository, so cloning it yields a
/// checkout with no commits and no upstream — and every later sync fails at
/// "no upstream configured" without ever explaining why. This is also the first
/// moment a commit is attempted on a new machine, which is precisely where a
/// missing git identity strands people, so both are settled here.
fn publish_initial(root: &Path) -> Result<()> {
    let has_upstream = crate::proc::quiet("git")
        .args(["rev-parse", "--abbrev-ref", "@{u}"])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_upstream {
        return Ok(());
    }

    ensure_identity(root)?;

    // A clone of an empty repository can land on whatever default the server
    // advertises; name it explicitly so every machine agrees.
    let _ = run("git", &["checkout", "-B", "main"], Some(root));
    let _ = run("git", &["add", "-A"], Some(root));

    let dirty = run("git", &["status", "--porcelain"], Some(root))?;
    if !dirty.trim().is_empty() {
        run(
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "Start of vault"],
            Some(root),
        )?;
    }

    match run("git", &["push", "-u", "origin", "main"], Some(root)) {
        Ok(_) => Ok(()),
        Err(e) => {
            // GH007: the account blocks pushes that would expose a private
            // email. An identity being *present* is not the same as it being
            // pushable, so the check above passes and the push still fails —
            // the same wall one step later. Swap in the noreply address, which
            // is public by construction, and try once more.
            let text = e.to_string();
            if text.contains("GH007") || text.contains("private email") {
                use_noreply_email(root)?;
                // Changing the config does not touch the commit already made
                // under the old address, and it is the commit the server
                // objects to. Re-author it — safe here precisely because this
                // runs once, on the very first commit of a new vault.
                run(
                    "git",
                    &["-c", "commit.gpgsign=false", "commit", "--amend", "--no-edit", "--reset-author"],
                    Some(root),
                )?;
                run("git", &["push", "-u", "origin", "main"], Some(root))?;
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// Point this clone at the address GitHub hands out for exactly this purpose.
fn use_noreply_email(root: &Path) -> Result<()> {
    let caps = capabilities();
    let login = caps
        .login
        .filter(|_| caps.gh_authenticated)
        .ok_or_else(|| Error::Other(
            "push was refused for exposing a private email, and gh cannot supply the noreply address — sign in with `gh auth login`".into(),
        ))?;
    let id = run("gh", &["api", "user", "--jq", ".id"], None)?;
    run(
        "git",
        &["config", "user.email", &format!("{id}+{login}@users.noreply.github.com")],
        Some(root),
    )?;
    Ok(())
}

/// Borrow the identity from `gh` when git has none.
///
/// Without this the first commit fails with "Author identity unknown", and on a
/// machine set up only through `gh auth login` that is the default state —
/// authenticating configures credentials but not identity. Set on this clone
/// only; an installer has no business rewriting what every other repository on
/// the machine commits under.
fn ensure_identity(root: &Path) -> Result<()> {
    // `--local`, not `--get`. The plain form reads the global config too, so a
    // machine with any global identity satisfied this check and skipped
    // everything below — including the noreply address. The first push then
    // died on GH007 ("would publish a private email"), which is the exact wall
    // install.ps1 already learned to avoid, reached through a different door:
    // setup succeeded, the watcher committed, and nothing ever left the machine.
    let have_local = |key: &str| {
        crate::proc::quiet("git")
            .args(["config", "--local", "--get", key])
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
            .unwrap_or(false)
    };
    if have_local("user.name") && have_local("user.email") {
        return Ok(());
    }

    let caps = capabilities();
    let login = match caps.login {
        Some(l) if caps.gh_authenticated => l,
        _ => {
            // No gh to ask. A global identity is then the best available, and
            // may well be fine — it is only private-email accounts that GH007
            // rejects, and the failure says so clearly when it happens.
            let have_any = |key: &str| {
                crate::proc::quiet("git")
                    .args(["config", "--get", key])
                    .current_dir(root)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
                    .unwrap_or(false)
            };
            if have_any("user.name") && have_any("user.email") {
                return Ok(());
            }
            return Err(Error::Other(
                "git has no user.name/user.email and gh cannot supply one — \
                 set them with `git config --global user.name` and `user.email`"
                    .into(),
            ));
        }
    };
    let id = run("gh", &["api", "user", "--jq", ".id"], None)?;
    let name = run("gh", &["api", "user", "--jq", ".name // .login"], None)
        .unwrap_or_else(|_| login.clone());

    // Written to the vault only, never globally: an installer has no business
    // changing the identity every other repository on the machine commits
    // under. The noreply address is the one GitHub hands out, so a push is
    // never rejected for exposing a private email (GH007).
    run("git", &["config", "user.name", &name], Some(root))?;
    run(
        "git",
        &["config", "user.email", &format!("{id}+{login}@users.noreply.github.com")],
        Some(root),
    )?;
    Ok(())
}

/// Refuse to write into somewhere that already has something in it.
///
/// Cloning into a non-empty directory fails anyway, but it fails after the
/// repository has been created on GitHub, leaving an empty repo behind and a
/// confusing error. Check first.
fn guard_destination(destination: &Path) -> Result<()> {
    if destination.exists() {
        let empty = std::fs::read_dir(destination)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(Error::Other(format!(
                "{} already exists and is not empty",
                destination.display()
            )));
        }
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Give a fresh vault the two things it cannot work without, and nothing else.
///
/// `.gitattributes` matters more than it looks: without it, Git for Windows'
/// default `core.autocrlf` rewrites line endings on checkout, and the same note
/// then differs byte-for-byte between machines and churns on every sync.
fn seed(root: &Path) -> Result<()> {
    let data = root.join("data");
    std::fs::create_dir_all(&data)?;
    let keep = data.join(".gitkeep");
    if !keep.exists() {
        std::fs::write(&keep, b"")?;
    }

    let attributes = root.join(".gitattributes");
    if !attributes.exists() {
        std::fs::write(
            &attributes,
            b"# Normalise line endings for text so the same note does not differ\n\
              # byte-for-byte between machines and churn on every sync.\n\
              * text=auto eol=lf\n\
              *.md text eol=lf\n\
              \n\
              # CRLF is significant in these; never convert.\n\
              *.bat  -text\n\
              *.cmd  -text\n\
              *.ps1  -text\n\
              *.reg  -text\n\
              *.pem  -text\n\
              *.crt  -text\n\
              *.key  -text\n",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_creates_what_a_vault_needs_and_is_idempotent() {
        let dir = std::env::temp_dir().join("jotbay-seed-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        seed(&dir).unwrap();
        assert!(dir.join("data/.gitkeep").exists());
        let attrs = std::fs::read_to_string(dir.join(".gitattributes")).unwrap();
        assert!(attrs.contains("eol=lf"));

        // A second run must not clobber a vault someone has customised.
        std::fs::write(dir.join(".gitattributes"), b"# mine\n").unwrap();
        seed(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitattributes")).unwrap(),
            "# mine\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_empty_destination_is_refused_before_anything_is_created() {
        let dir = std::env::temp_dir().join("jotbay-guard-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("something"), b"x").unwrap();

        assert!(guard_destination(&dir).is_err(), "must refuse a non-empty dir");

        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(guard_destination(&empty).is_ok(), "an empty dir is fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adopting_something_that_is_not_a_repo_fails_clearly() {
        let dir = std::env::temp_dir().join("jotbay-adopt-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = adopt(&dir).unwrap_err().to_string();
        assert!(err.contains("not a git repository"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
