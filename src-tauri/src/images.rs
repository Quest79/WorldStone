//! The Docker images this instance uses, named for what they are, and removing old server builds.
//!
//! Each server has its own image, named `worldstone/<edition>-<role>` (for example
//! `worldstone/vanilla-realm` and `worldstone/vanilla-world`), so Docker Desktop says what every
//! row is. This module reads the real images and the real containers and says, for each image,
//! what it is and what uses it. Builds that a newer one replaced, and images from before servers
//! had their own, are listed as old; only old images that no container uses can be removed, one
//! `docker image rm` each and never `--force`.

use crate::{
    builder, docker,
    process::{capture, CommandSpec},
    recipe::Recipe,
    tasks::Task,
};
use serde::Serialize;
use std::time::Duration;

/// What Docker calls the name and tag of an image that lost its name.
const UNNAMED: &str = "<none>";

/// One image as Docker reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// What `docker image rm` is given: `name:tag`, or the image id for an unnamed image.
    pub reference: String,
    pub name: String,
    pub tag: String,
    /// The full image id (`sha256:…`); two tags of one image share it.
    pub id: String,
    pub size: u64,
    pub created: String,
}

/// A container and the id of the image it was made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub container: String,
    pub image_id: String,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ImageKind {
    /// A server's image, the one its container runs.
    Current,
    /// An image a newer build replaced, or one from before each server had its own.
    Old,
    /// The database server, downloaded as it is.
    Database,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRow {
    pub reference: String,
    pub name: String,
    pub tag: String,
    /// The first 12 characters of the image id, as Docker Desktop shows it.
    pub id: String,
    pub kind: ImageKind,
    /// The heading: which server this is ("Realm server"), or "Old build".
    pub title: String,
    /// What this image is, in words.
    pub what: String,
    pub size_bytes: u64,
    pub created: String,
    /// The containers made from this image.
    pub used_by: Vec<String>,
    /// Whether the app may remove it: an old image that nothing uses.
    pub removable: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImagesStatus {
    /// Set when Docker could not be asked; the list is then empty and says nothing about the images.
    pub note: Option<String>,
    pub images: Vec<ImageRow>,
}

fn short_id(id: &str) -> String {
    id.trim_start_matches("sha256:").chars().take(12).collect()
}

fn used_by(users: &[User], id: &str) -> Vec<String> {
    let mut names: Vec<String> = users.iter().filter(|u| u.image_id == id).map(|u| u.container.clone()).collect();
    names.sort();
    names.dedup();
    names
}

fn row(found: &Found, kind: ImageKind, title: &str, what: String, users_of: Vec<String>, removable: bool) -> ImageRow {
    ImageRow {
        reference: found.reference.clone(),
        name: found.name.clone(),
        tag: found.tag.clone(),
        id: short_id(&found.id),
        kind,
        title: title.to_string(),
        what,
        size_bytes: found.size,
        created: found.created.clone(),
        used_by: users_of,
        removable,
    }
}

/// Names the images. Pure, so every wording and every removal rule is testable.
///
/// `servers` are the servers' own images as `(label, image)` in start order and `expected` is how many
/// there should be; `old` are the images a newer build replaced or that predate them; `database` is
/// the downloaded database image. Nothing is called removable until every server has its own image:
/// until then an old one may be the only build that works.
pub fn describe(edition: &str, servers: &[(String, Found)], expected: usize, old: &[Found], database: Option<&Found>, users: &[User]) -> Vec<ImageRow> {
    let mut rows = Vec::new();
    for (label, found) in servers {
        let what = format!("The {edition} {}: the image its container runs from.", label.to_lowercase());
        rows.push(row(found, ImageKind::Current, label, what, used_by(users, &found.id), false));
    }
    let complete = servers.len() == expected;
    let mut old: Vec<&Found> = old.iter().filter(|f| !servers.iter().any(|(_, s)| s.id == f.id)).collect();
    old.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.reference.cmp(&b.reference)));
    for found in old {
        let users_of = used_by(users, &found.id);
        let origin = if found.tag == UNNAMED {
            "A build that a newer one replaced".to_string()
        } else {
            format!("{} from before each server had its own image", found.reference)
        };
        let what = if users_of.is_empty() {
            if complete {
                format!("{origin}. Nothing uses it any more, so it can be removed.")
            } else {
                format!("{origin}. Build the server again first; it is left alone until then.")
            }
        } else {
            format!("{origin}. {} still uses it. Start the servers again to move them to the current images.", users_of.join(", "))
        };
        let removable = complete && users_of.is_empty();
        rows.push(row(found, ImageKind::Old, "Old build", what, users_of, removable));
    }
    if let Some(found) = database {
        let what = "The database server (the public MariaDB image, named for this server). The database container runs from this.".to_string();
        rows.push(row(found, ImageKind::Database, "Database server", what, used_by(users, &found.id), false));
    }
    rows
}

/// Splits `repository:tag` at its last colon (a registry port would contain an earlier one).
fn split_reference(text: &str) -> Option<(&str, &str)> {
    let (name, tag) = text.rsplit_once(':')?;
    (!name.is_empty() && !tag.is_empty() && tag != UNNAMED).then_some((name, tag))
}

async fn inspect_reference(reference: &str, name: &str, tag: &str) -> Option<Found> {
    let image = docker::image(reference).await?;
    Some(Found { reference: reference.to_string(), name: name.to_string(), tag: tag.to_string(), id: image.id, size: image.size, created: image.created })
}

async fn inspect(name: &str, tag: &str) -> Option<Found> {
    inspect_reference(&format!("{name}:{tag}"), name, tag).await
}

/// The containers on this machine and the image each was made from.
pub(crate) async fn container_users() -> Result<Vec<User>, String> {
    Ok(docker::containers(true).await?.into_iter().map(|c| User { container: c.name, image_id: c.image_id }).collect())
}

/// Every tag of one repository, as images.
async fn tagged(repo: &str) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    for reference in docker::image_tags(repo).await? {
        if let Some((name, tag)) = split_reference(reference.trim()).filter(|(name, _)| *name == repo) {
            found.extend(inspect(name, tag).await);
        }
    }
    Ok(found)
}

/// The images of this instance's builds that lost their name, which is what a newer build leaves behind.
async fn unnamed(instance: &str) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    for id in docker::unnamed_images(&format!("{}={instance}", builder::LABEL_INSTANCE)).await? {
        found.extend(inspect_reference(&id, UNNAMED, UNNAMED).await);
    }
    Ok(found)
}

/// The images of this instance as they are now.
pub async fn status(instance: &str, recipe: &Recipe) -> ImagesStatus {
    if let Err(reason) = builder::engine().await {
        return ImagesStatus { note: Some(reason), images: Vec::new() };
    }
    match read(instance, recipe).await {
        Ok(images) => ImagesStatus { note: None, images },
        Err(note) => ImagesStatus { note: Some(note), images: Vec::new() },
    }
}

async fn read(instance: &str, recipe: &Recipe) -> Result<Vec<ImageRow>, String> {
    let mut servers = Vec::new();
    for service in &recipe.run.services {
        if let Some(found) = inspect(&builder::image_repo(instance, recipe, service), "latest").await {
            servers.push((service.label.clone(), found));
        }
    }
    let mut old = tagged(&builder::legacy_repo(instance)).await?;
    old.extend(unnamed(instance).await?);
    // Its own name once the database step has given it one; the public name until then.
    let mut database = None;
    for reference in [builder::database_image(instance, recipe), recipe.database.image.clone()] {
        if let Some((name, tag)) = split_reference(&reference) {
            database = inspect(name, tag).await;
            if database.is_some() {
                break;
            }
        }
    }
    let users = container_users().await?;
    Ok(describe(&recipe.edition, &servers, recipe.run.services.len(), &old, database.as_ref(), &users))
}

/// What removing old builds would do, in words, for the button's description.
pub fn removable(images: &[ImageRow]) -> Vec<&ImageRow> {
    images.iter().filter(|i| i.removable).collect()
}

/// Removes the old images that nothing uses. The servers' current images and the database image
/// are never touched, and nothing is forced: Docker refuses an image in use.
pub async fn remove_old(task: &mut Task, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Looking at the images and the containers");
    let now = status(instance, recipe).await;
    if let Some(note) = now.note {
        return Err(note);
    }
    let victims = removable(&now.images);
    if victims.is_empty() {
        task.info("There is no old build to remove: every image is either current or still in use.");
        return Ok(());
    }
    let mut removed = 0;
    for image in victims {
        task.step(&format!("Removing {} ({})", image.reference, image.id));
        let spec = CommandSpec::new("docker", &["image", "rm", &image.reference]).timeout(Duration::from_secs(2 * 60));
        let out = capture(&spec).await;
        if !out.success {
            return Err(format!("Docker did not remove {}: {}", image.reference, out.text));
        }
        task.info(&format!("Removed {}", image.reference));
        removed += 1;
    }
    task.info(&format!("Removed {removed} old image{}. Space is only freed for layers no other image shares.", if removed == 1 { "" } else { "s" }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, tag: &str, id: &str) -> Found {
        let reference = if tag == UNNAMED { id.to_string() } else { format!("{name}:{tag}") };
        Found { reference, name: name.into(), tag: tag.into(), id: format!("sha256:{id}"), size: 152_000_000, created: "2026-09-29T00:00:00Z".into() }
    }

    fn user(container: &str, id: &str) -> User {
        User { container: container.into(), image_id: format!("sha256:{id}") }
    }

    fn servers() -> Vec<(String, Found)> {
        vec![
            ("Realm server".into(), found("worldstone/vanilla-realm", "latest", "aaaa11112222")),
            ("World server".into(), found("worldstone/vanilla-world", "latest", "bbbb33334444")),
        ]
    }

    #[test]
    fn each_server_image_says_which_server_and_which_edition_it_is() {
        let users = [user("worldstone-mangos-zero-realmd", "aaaa11112222"), user("worldstone-mangos-zero-mangosd", "bbbb33334444")];
        let rows = describe("vanilla", &servers(), 2, &[], None, &users);
        assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Realm server", "World server"]);
        assert_eq!(rows.iter().map(|r| r.reference.as_str()).collect::<Vec<_>>(), ["worldstone/vanilla-realm:latest", "worldstone/vanilla-world:latest"]);
        assert!(rows.iter().all(|r| r.kind == ImageKind::Current && !r.removable));
        assert!(rows[0].what.starts_with("The vanilla realm server"), "{}", rows[0].what);
        assert_eq!(rows[1].used_by, ["worldstone-mangos-zero-mangosd"]);
        assert_eq!(rows[1].id, "bbbb33334444");
    }

    #[test]
    fn a_replaced_build_and_an_image_from_before_are_old_and_removable_when_nothing_uses_them() {
        let old = [found("worldstone/mangos-zero", "latest", "cccc"), found("worldstone/mangos-zero", "d77f7e608343", "cccc"), found(UNNAMED, UNNAMED, "dddd")];
        let rows = describe("vanilla", &servers(), 2, &old, None, &[]);
        let olds: Vec<&ImageRow> = rows.iter().filter(|r| r.kind == ImageKind::Old).collect();
        assert_eq!(olds.len(), 3);
        assert!(olds.iter().all(|r| r.removable && r.title == "Old build"));
        assert!(olds.iter().any(|r| r.what.contains("from before each server had its own image")));
        assert!(olds.iter().any(|r| r.what.contains("replaced") && r.reference == "dddd"), "an unnamed image is removed by its id");
    }

    #[test]
    fn an_old_image_that_a_container_still_runs_is_kept_and_says_who() {
        let old = [found("worldstone/mangos-zero", "latest", "cccc")];
        let users = [user("worldstone-mangos-zero-realmd", "cccc"), user("worldstone-mangos-zero-mangosd", "cccc")];
        let rows = describe("vanilla", &servers(), 2, &old, None, &users);
        let old = rows.iter().find(|r| r.kind == ImageKind::Old).unwrap();
        assert!(!old.removable);
        assert!(old.what.contains("worldstone-mangos-zero-mangosd, worldstone-mangos-zero-realmd still uses"), "{}", old.what);
    }

    #[test]
    fn nothing_old_is_removable_until_every_server_has_its_own_image() {
        let old = [found("worldstone/mangos-zero", "latest", "cccc")];
        let rows = describe("vanilla", &servers()[..1], 2, &old, None, &[]);
        let old = rows.iter().find(|r| r.kind == ImageKind::Old).unwrap();
        assert!(!old.removable && old.what.contains("Build the server again first"), "{}", old.what);
        let none = describe("vanilla", &[], 2, &old_only(), None, &[]);
        assert!(none.iter().all(|r| !r.removable));
    }

    fn old_only() -> Vec<Found> {
        vec![found("worldstone/mangos-zero", "latest", "cccc")]
    }

    #[test]
    fn a_current_image_is_never_listed_as_old_even_if_it_shares_an_id() {
        let old = [found("worldstone/mangos-zero", "latest", "aaaa11112222")];
        let rows = describe("vanilla", &servers(), 2, &old, None, &[]);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.kind == ImageKind::Current));
    }

    #[test]
    fn the_database_image_is_described_and_never_removable() {
        let db = Found { reference: "mariadb:11.4".into(), name: "mariadb".into(), tag: "11.4".into(), id: "sha256:70cc072b29b4ff".into(), size: 455_000_000, created: "x".into() };
        let rows = describe("vanilla", &[], 2, &[], Some(&db), &[user("worldstone-mangos-zero-db", "70cc072b29b4ff")]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind, rows[0].removable, rows[0].reference.as_str(), rows[0].title.as_str()), (ImageKind::Database, false, "mariadb:11.4", "Database server"));
        assert_eq!(rows[0].used_by, ["worldstone-mangos-zero-db"]);
    }

    #[test]
    fn references_split_at_the_last_colon_and_untagged_images_are_skipped() {
        assert_eq!(split_reference("mariadb:11.4"), Some(("mariadb", "11.4")));
        assert_eq!(split_reference("localhost:5000/x:1"), Some(("localhost:5000/x", "1")));
        assert_eq!(split_reference("worldstone/x:<none>"), None);
        assert_eq!(split_reference("nocolon"), None);
    }
}
