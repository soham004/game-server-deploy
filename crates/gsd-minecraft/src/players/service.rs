//! Players of one server: the view, and changes made the way the server's state allows.
//!
//! - **Running, 1.21.9 or later:** the management protocol. If it can't be reached, the console, for what
//!   the console can say.
//! - **Running, older:** console commands (`op`, `whitelist add`, `ban`, …).
//! - **Stopped:** the server's files, which it reads when it starts. A name becomes an account first: the
//!   server's own name cache, else Mojang (online mode), else the offline rule.
//! - **Starting or stopping:** nothing; it is about to be one of the above.
//!
//! Every change is checked against the server's own lists afterwards (the console can't answer, so for it
//! the lists are watched for a few seconds): an action only succeeds once the server shows it done.

use std::{collections::HashMap, time::Duration};

use chrono::Utc;
use gsd_core::{
    files::{FilesError, SafeRelPath, ServerFiles},
    ids::InstanceId,
    instances::{Instance, InstanceError, InstanceService},
    players::{PlayerHistory, Session},
    store::RepoError,
    supervisor::ServerState,
};
use serde::Serialize;
use serde_json::Value;

use super::{
    action::{InvalidAction, PLAYERS_MANAGE_MAX_LEVEL, PlayerAction, ip_address},
    lists::{self, BanDetails, CacheEntry, Player},
    mojang::{Mojang, MojangError},
    names::{normalize_uuid, offline_uuid},
    view::{self, ActionChannel, PlayersView, ServerSettings, Sources},
};
use crate::{
    control::{self, Channel, ControlError, mcsmp::McsmpError},
    properties::PropertiesDoc,
    settings::MinecraftServer,
};

const PROPERTIES: &str = "server.properties";
/// How long the console's changes are watched for, and how often.
const CONFIRM_WAIT: Duration = Duration::from_secs(6);
const CONFIRM_EVERY: Duration = Duration::from_millis(250);
/// Statistics files read for one view: the players listed first.
const MAX_STATS_READ: usize = 100;
/// Visits shown for one player.
pub const SESSIONS_SHOWN: u32 = 50;

#[derive(Debug, thiserror::Error)]
pub enum PlayersError {
    #[error(transparent)]
    Invalid(#[from] InvalidAction),
    #[error("wait until the server has started or stopped")]
    Busy,
    #[error("the server has to be running for that")]
    NotRunning,
    #[error("{0} needs Minecraft 1.21.9 or later while the server runs, or a stopped server")]
    NeedsManagement(&'static str),
    #[error("operator levels above {PLAYERS_MANAGE_MAX_LEVEL} need the right to run any console command")]
    LevelNotAllowed,
    #[error("no Minecraft account is called \"{0}\"")]
    NoAccount(String),
    #[error("{0} isn't on the server")]
    NotOnline(String),
    #[error("the server refused: {0}")]
    Refused(String),
    #[error("the server didn't confirm it; its console may say why")]
    NotConfirmed,
    #[error("could not look the name up: {0}")]
    Mojang(#[from] MojangError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error("could not read the player history")]
    History(#[from] RepoError),
}

/// What an action did, and the lists after it.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    /// How it was made: the management protocol, the console, or the files.
    pub via: ActionChannel,
    pub players: PlayersView,
}

#[derive(Debug, Clone)]
pub struct PlayerService {
    instances: InstanceService,
    history: PlayerHistory,
    mojang: Mojang,
}

impl PlayerService {
    pub fn new(instances: InstanceService, history: PlayerHistory, mojang: Mojang) -> Self {
        Self {
            instances,
            history,
            mojang,
        }
    }

    pub fn mojang(&self) -> &Mojang {
        &self.mojang
    }

    pub async fn view(&self, id: InstanceId) -> Result<PlayersView, PlayersError> {
        let instance = self.instances.get(id).await?;
        let channel = channel_now(&self.instances.state(id), &instance);
        self.view_of(&instance, channel).await
    }

    /// One player's latest visits, newest first.
    pub async fn sessions(&self, id: InstanceId, name: &str) -> Result<Vec<Session>, PlayersError> {
        self.instances.get(id).await?;
        Ok(self.history.sessions(id, name, SESSIONS_SHOWN).await?)
    }

    /// The server's default operator level, for deciding who may give it.
    pub async fn default_op_level(&self, id: InstanceId) -> Result<u8, PlayersError> {
        let instance = self.instances.get(id).await?;
        let files = ServerFiles::open(&instance).await?;
        let properties = read_optional(&files, PROPERTIES).await?;
        Ok(ServerSettings::from_properties(properties.as_deref()).default_op_level)
    }

    /// Does `action`, as `author` (a username: bans name it as their source). `may_give_any_level` is
    /// whether they may make operators above level 2.
    pub async fn apply(
        &self,
        id: InstanceId,
        action: &PlayerAction,
        author: &str,
        may_give_any_level: bool,
    ) -> Result<Applied, PlayersError> {
        action.validate(Utc::now())?;
        let instance = self.instances.get(id).await?;
        let files = ServerFiles::open(&instance).await?;
        let settings = ServerSettings::from_properties(read_optional(&files, PROPERTIES).await?.as_deref());
        if action
            .op_level(settings.default_op_level)
            .is_some_and(|level| level > PLAYERS_MANAGE_MAX_LEVEL)
            && !may_give_any_level
        {
            return Err(PlayersError::LevelNotAllowed);
        }
        let state = self.instances.state(id);
        let Some(channel) = channel_now(&state, &instance) else {
            return Err(PlayersError::Busy);
        };
        if channel == ActionChannel::Files && self.instances.is_busy(id) {
            return Err(PlayersError::Busy);
        }
        let via = match channel {
            ActionChannel::Management => {
                self.through_management(&instance, action, author, &settings)
                    .await?
            }
            ActionChannel::Console => {
                self.through_console(id, action, author, &settings).await?;
                ActionChannel::Console
            }
            ActionChannel::Files => {
                if action.needs_running_server() {
                    return Err(PlayersError::NotRunning);
                }
                self.through_files(&files, action, author, &settings).await?;
                ActionChannel::Files
            }
        };
        let players = self.confirmed(&instance, action).await?;
        Ok(Applied { via, players })
    }

    async fn view_of(
        &self,
        instance: &Instance,
        channel: Option<ActionChannel>,
    ) -> Result<PlayersView, PlayersError> {
        let id = instance.definition.id;
        let files = ServerFiles::open(instance).await?;
        let online = if channel.is_some_and(|channel| channel != ActionChannel::Files) {
            self.instances.snapshot(id).await?.players
        } else {
            Vec::new()
        };
        let sources = Sources {
            properties: read_optional(&files, PROPERTIES).await?,
            ops: read_optional(&files, lists::OPS).await?,
            allowlist: read_optional(&files, lists::ALLOWLIST).await?,
            bans: read_optional(&files, lists::BANS).await?,
            ip_bans: read_optional(&files, lists::IP_BANS).await?,
            user_cache: read_optional(&files, lists::USER_CACHE).await?,
            online,
            history: self.history.players(id, Utc::now()).await?,
        };
        let level_name = ServerSettings::from_properties(sources.properties.as_deref()).level_name;
        let mut players = view::build(channel, sources);
        let played = played_from_stats(&files, &level_name, &players).await;
        view::use_game_play_time(&mut players, &played);
        Ok(players)
    }

    /// Calls the management protocol; if it can't be reached, the console takes over where it can.
    async fn through_management(
        &self,
        instance: &Instance,
        action: &PlayerAction,
        author: &str,
        settings: &ServerSettings,
    ) -> Result<ActionChannel, PlayersError> {
        let mut client = match control::management(instance).await {
            Ok(client) => client,
            Err(error)
                if action
                    .needs_more_than_console(settings.default_op_level)
                    .is_none() =>
            {
                tracing::warn!(instance = %instance.definition.id, %error, "management protocol unreachable; using the console");
                self.through_console(instance.definition.id, action, author, settings)
                    .await?;
                return Ok(ActionChannel::Console);
            }
            Err(error) => return Err(error.into()),
        };
        let (method, params) = action.management_call(author);
        let reply = client
            .call(method, Some(params))
            .await
            .map_err(|error| match error {
                McsmpError::Rpc { message, .. } => PlayersError::Refused(message),
                other => PlayersError::Control(ControlError::Mcsmp(other)),
            })?;
        // A kick names who it kicked; nobody means they weren't on.
        if let PlayerAction::Kick { name, .. } = action
            && reply.as_array().is_some_and(Vec::is_empty)
        {
            return Err(PlayersError::NotOnline(name.clone()));
        }
        Ok(ActionChannel::Management)
    }

    async fn through_console(
        &self,
        id: InstanceId,
        action: &PlayerAction,
        author: &str,
        settings: &ServerSettings,
    ) -> Result<(), PlayersError> {
        if let Some(what) = action.needs_more_than_console(settings.default_op_level) {
            return Err(PlayersError::NeedsManagement(what));
        }
        if let PlayerAction::Kick { name, .. } | PlayerAction::Message { name, .. } = action
            && !self
                .instances
                .snapshot(id)
                .await?
                .players
                .iter()
                .any(|online| online.eq_ignore_ascii_case(name))
        {
            return Err(PlayersError::NotOnline(name.clone()));
        }
        self.instances
            .send_command(id, &action.console_command(), Some(author.to_owned()))
            .await?;
        Ok(())
    }

    /// Edits the stopped server's files.
    async fn through_files(
        &self,
        files: &ServerFiles,
        action: &PlayerAction,
        author: &str,
        settings: &ServerSettings,
    ) -> Result<(), PlayersError> {
        let now = Utc::now();
        let edit = |list: &'static str| ListEdit { files, list };
        match action {
            PlayerAction::Op { name, level } => {
                let player = self.account(files, name, settings).await?;
                let level = level.unwrap_or(settings.default_op_level);
                edit(lists::OPS)
                    .apply(|entries| lists::upsert(entries, &player, lists::operator_entry(&player, level)))
                    .await
            }
            PlayerAction::Allow { name } => {
                let player = self.account(files, name, settings).await?;
                edit(lists::ALLOWLIST)
                    .apply(|entries| lists::upsert(entries, &player, lists::player_entry(&player)))
                    .await
            }
            PlayerAction::Ban {
                name,
                reason,
                expires,
            } => {
                let player = self.account(files, name, settings).await?;
                let details = BanDetails {
                    reason: reason.as_deref().map(str::trim).filter(|r| !r.is_empty()),
                    source: author,
                    expires: *expires,
                    now,
                };
                edit(lists::BANS)
                    .apply(|entries| lists::upsert(entries, &player, lists::ban_entry(&player, &details)))
                    .await
            }
            PlayerAction::BanIp {
                target,
                reason,
                expires,
            } => {
                let ip = ip_address(target)?;
                let details = BanDetails {
                    reason: reason.as_deref().map(str::trim).filter(|r| !r.is_empty()),
                    source: author,
                    expires: *expires,
                    now,
                };
                edit(lists::IP_BANS)
                    .apply(|entries| {
                        lists::remove_ip(entries, &ip);
                        entries.push(lists::ip_ban_entry(&ip, &details));
                    })
                    .await
            }
            PlayerAction::Deop { name } => {
                edit(lists::OPS)
                    .apply(|entries| {
                        lists::remove_named(entries, name);
                    })
                    .await
            }
            PlayerAction::Disallow { name } => {
                edit(lists::ALLOWLIST)
                    .apply(|entries| {
                        lists::remove_named(entries, name);
                    })
                    .await
            }
            PlayerAction::Pardon { name } => {
                edit(lists::BANS)
                    .apply(|entries| {
                        lists::remove_named(entries, name);
                    })
                    .await
            }
            PlayerAction::PardonIp { ip } => {
                let ip = ip_address(ip)?;
                edit(lists::IP_BANS)
                    .apply(|entries| {
                        lists::remove_ip(entries, &ip);
                    })
                    .await
            }
            PlayerAction::Allowlist { on } => {
                set_property(files, "white-list", if *on { "true" } else { "false" }).await
            }
            PlayerAction::Kick { .. } | PlayerAction::Message { .. } => Err(PlayersError::NotRunning),
        }
    }

    /// The account a name stands for on this server. One found elsewhere than the server's name cache
    /// goes into it, as the game does after a lookup: a running 26.3 server resolves a name it hasn't
    /// cached by lowercasing it (offline mode: a different UUID), so it couldn't find this entry again,
    /// and couldn't lift the ban or take the operator away.
    async fn account(
        &self,
        files: &ServerFiles,
        name: &str,
        settings: &ServerSettings,
    ) -> Result<Player, PlayersError> {
        let cache: Vec<CacheEntry> = lists::read(read_optional(files, lists::USER_CACHE).await?.as_deref());
        if let Some(entry) = cache.iter().find(|entry| entry.name.eq_ignore_ascii_case(name))
            && let Some(uuid) = normalize_uuid(&entry.uuid)
        {
            return Ok(Player {
                uuid,
                name: entry.name.clone(),
            });
        }
        let player = if settings.online_mode {
            let profile = self
                .mojang
                .lookup(name)
                .await?
                .ok_or_else(|| PlayersError::NoAccount(name.to_owned()))?;
            Player {
                uuid: profile.uuid,
                name: profile.name,
            }
        } else {
            Player {
                uuid: offline_uuid(name),
                name: name.to_owned(),
            }
        };
        let now = Utc::now();
        ListEdit {
            files,
            list: lists::USER_CACHE,
        }
        .apply(|entries| lists::upsert(entries, &player, lists::cache_entry(&player, now)))
        .await?;
        Ok(player)
    }

    /// The lists once they show the action done. The files and the management protocol are done when
    /// they answer, but who is online comes from the console's leave lines, and console commands get no
    /// answer at all, so the lists are watched for a few seconds.
    async fn confirmed(
        &self,
        instance: &Instance,
        action: &PlayerAction,
    ) -> Result<PlayersView, PlayersError> {
        let id = instance.definition.id;
        let deadline = tokio::time::Instant::now() + CONFIRM_WAIT;
        loop {
            let channel = channel_now(&self.instances.state(id), instance);
            let players = self.view_of(instance, channel).await?;
            match action.is_done(&players) {
                None | Some(true) => return Ok(players),
                Some(false) if tokio::time::Instant::now() >= deadline => {
                    return Err(PlayersError::NotConfirmed);
                }
                Some(false) => tokio::time::sleep(CONFIRM_EVERY).await,
            }
        }
    }
}

/// How changes can be made in this state; `None` while it starts or stops.
pub(crate) fn channel_now(state: &ServerState, instance: &Instance) -> Option<ActionChannel> {
    match state {
        ServerState::Running { .. } => {
            let mc_version = MinecraftServer::from_table(&instance.definition.server)
                .map(|server| server.mc_version)
                .unwrap_or_default();
            Some(match control::channel_for(&mc_version) {
                Channel::Management => ActionChannel::Management,
                Channel::Rcon => ActionChannel::Console,
            })
        }
        ServerState::Starting { .. } | ServerState::Stopping { .. } => None,
        ServerState::Stopped { .. } | ServerState::Crashed { .. } => Some(ActionChannel::Files),
    }
}

/// One list file being changed: read with its revision, changed, written back unless it changed meanwhile.
struct ListEdit<'a> {
    files: &'a ServerFiles,
    list: &'static str,
}

impl ListEdit<'_> {
    async fn apply(self, change: impl FnOnce(&mut Vec<Value>)) -> Result<(), PlayersError> {
        let path = SafeRelPath::parse(self.list).map_err(FilesError::from)?;
        let (text, revision) = match self.files.read_text(path.clone()).await {
            Ok(file) => (Some(file.content), Some(file.revision)),
            Err(FilesError::NotFound(_)) => (None, None),
            Err(error) => return Err(error.into()),
        };
        let mut entries = lists::values(text.as_deref());
        change(&mut entries);
        self.files
            .write_text(path, lists::render(&entries), revision)
            .await?;
        Ok(())
    }
}

async fn set_property(files: &ServerFiles, key: &str, value: &str) -> Result<(), PlayersError> {
    let path = SafeRelPath::parse(PROPERTIES).map_err(FilesError::from)?;
    let (mut doc, revision) = match files.read_text(path.clone()).await {
        Ok(file) => (PropertiesDoc::parse(&file.content), Some(file.revision)),
        Err(FilesError::NotFound(_)) => (PropertiesDoc::parse(""), None),
        Err(error) => return Err(error.into()),
    };
    doc.set(key, value);
    files.write_text(path, doc.render(), revision).await?;
    Ok(())
}

/// A file of the server's, or `None` if it has none.
async fn read_optional(files: &ServerFiles, name: &str) -> Result<Option<String>, PlayersError> {
    let path = SafeRelPath::parse(name).map_err(FilesError::from)?;
    match files.read_text(path).await {
        Ok(file) => Ok(Some(file.content)),
        Err(FilesError::NotFound(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The game's own play time of the players listed first. A statistics file that can't be read is skipped.
async fn played_from_stats(
    files: &ServerFiles,
    level_name: &str,
    view: &PlayersView,
) -> HashMap<String, u64> {
    let mut played = HashMap::new();
    for uuid in view
        .known
        .iter()
        .filter_map(|player| player.uuid.as_deref())
        .take(MAX_STATS_READ)
    {
        let Some(uuid) = normalize_uuid(uuid) else {
            continue;
        };
        let Ok(path) = SafeRelPath::parse(&format!("{level_name}/stats/{uuid}.json")) else {
            continue;
        };
        if let Ok(file) = files.read_text(path).await
            && let Some(seconds) = view::played_seconds_in_stats(&file.content)
        {
            played.insert(uuid, seconds);
        }
    }
    played
}
