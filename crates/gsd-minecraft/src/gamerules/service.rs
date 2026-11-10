//! Game rules of one server: the view, and changes made the way its state allows (see the module).
//! A change only succeeds once the server shows it.

use std::{collections::BTreeMap, time::Duration};

use gsd_core::{
    backups::wait_for_save,
    events::EventBus,
    files::{FilesError, SafeRelPath, ServerFiles},
    ids::InstanceId,
    instances::{Instance, InstanceService},
};
use serde::Serialize;
use serde_json::{Value, json};

use super::{GameruleError, GamerulesView, RuleValue, ValuesFrom, describe, sorted, world};
use crate::{
    control,
    players::{ActionChannel, service::channel_now},
    properties::PropertiesDoc,
    worlds,
};

/// How long the console path waits for the server's save after its commands.
const SAVE_WAIT: Duration = Duration::from_secs(60);
/// Changes accepted at once; there are about sixty rules.
const MAX_CHANGES: usize = 200;
/// Makes the server write its world, game rules included.
const SAVE_COMMAND: &str = "save-all";

/// What a change did, and the rules after it.
#[derive(Debug, Clone, Serialize)]
pub struct Updated {
    pub via: ActionChannel,
    pub gamerules: GamerulesView,
}

#[derive(Debug, Clone)]
pub struct GameruleService {
    instances: InstanceService,
    events: EventBus,
}

impl GameruleService {
    pub fn new(instances: InstanceService, events: EventBus) -> Self {
        Self { instances, events }
    }

    pub async fn view(&self, id: InstanceId) -> Result<GamerulesView, GameruleError> {
        let instance = self.instances.get(id).await?;
        let channel = channel_now(&self.instances.state(id), &instance);
        if channel == Some(ActionChannel::Management) {
            match self.live(&instance).await {
                Ok(view) => return Ok(view),
                Err(error) => {
                    tracing::warn!(%id, %error, "management protocol unreachable; showing the saved game rules");
                }
            }
        }
        self.saved(&instance, channel).await
    }

    /// Sets rules by name, as `author` (shown next to console commands).
    pub async fn update(
        &self,
        id: InstanceId,
        changes: BTreeMap<String, RuleValue>,
        author: &str,
    ) -> Result<Updated, GameruleError> {
        if changes.is_empty() || changes.len() > MAX_CHANGES {
            return Err(GameruleError::NothingToChange);
        }
        let instance = self.instances.get(id).await?;
        let channel = channel_now(&self.instances.state(id), &instance).ok_or(GameruleError::Busy)?;
        let current = self.view(id).await?;
        if current.values.is_none() {
            return Err(GameruleError::NoWorld);
        }
        // Only rules the server has, with the kind of value it has: names come from the server, never the request.
        let changes: Vec<(String, RuleValue)> = changes
            .into_iter()
            .map(|(name, value)| {
                let rule = current
                    .rules
                    .iter()
                    .find(|rule| rule.name == name)
                    .ok_or_else(|| GameruleError::UnknownRule(name.clone()))?;
                if rule.kind != value.kind() || !value.fits_an_int() {
                    return Err(GameruleError::WrongType { name: name.clone() });
                }
                Ok((rule.name.clone(), value))
            })
            .collect::<Result<_, _>>()?;

        let via = match channel {
            ActionChannel::Management if current.values == Some(ValuesFrom::Server) => {
                self.through_management(&instance, &changes).await?;
                ActionChannel::Management
            }
            ActionChannel::Management | ActionChannel::Console => {
                self.through_console(id, &changes, author).await?;
                ActionChannel::Console
            }
            ActionChannel::Files => {
                if self.instances.is_busy(id) {
                    return Err(GameruleError::Busy);
                }
                let files = ServerFiles::open(&instance).await?;
                let (storage, _) = world::read(&files, &current.world)
                    .await?
                    .ok_or(GameruleError::NoWorld)?;
                world::write(&files, &current.world, storage, &changes).await?;
                ActionChannel::Files
            }
        };

        let gamerules = self.view(id).await?;
        let shown = |name: &str| {
            gamerules
                .rules
                .iter()
                .find(|rule| rule.name == name)
                .map(|rule| rule.value)
        };
        if changes.iter().any(|(name, value)| shown(name) != Some(*value)) {
            return Err(GameruleError::NotConfirmed);
        }
        Ok(Updated { via, gamerules })
    }

    async fn live(&self, instance: &Instance) -> Result<GamerulesView, GameruleError> {
        let mut client = control::management(instance).await?;
        let reply = client.call("minecraft:gamerules", None).await?;
        let rules = reply
            .as_array()
            .ok_or(GameruleError::Control(control::ControlError::Mcsmp(
                control::mcsmp::McsmpError::Invalid,
            )))?
            .iter()
            .filter_map(|rule| {
                let key = rule.get("key")?.as_str()?;
                let value = match rule.get("value")? {
                    Value::Bool(on) => RuleValue::Bool(*on),
                    number => RuleValue::Int(number.as_i64()?),
                };
                Some(describe(key.strip_prefix("minecraft:").unwrap_or(key), value))
            })
            .collect();
        Ok(GamerulesView {
            channel: Some(ActionChannel::Management),
            values: Some(ValuesFrom::Server),
            world: level_name(&ServerFiles::open(instance).await?).await?,
            rules: sorted(rules),
        })
    }

    async fn saved(
        &self,
        instance: &Instance,
        channel: Option<ActionChannel>,
    ) -> Result<GamerulesView, GameruleError> {
        let files = ServerFiles::open(instance).await?;
        let world = level_name(&files).await?;
        let stored = world::read(&files, &world).await?;
        let values = stored.as_ref().map(|_| {
            if channel == Some(ActionChannel::Files) {
                ValuesFrom::Files
            } else {
                ValuesFrom::LastSave
            }
        });
        let rules = stored
            .map(|(_, rules)| {
                rules
                    .into_iter()
                    .map(|(name, value)| describe(&name, value))
                    .collect()
            })
            .unwrap_or_default();
        Ok(GamerulesView {
            channel,
            values,
            world,
            rules: sorted(rules),
        })
    }

    async fn through_management(
        &self,
        instance: &Instance,
        changes: &[(String, RuleValue)],
    ) -> Result<(), GameruleError> {
        let mut client = control::management(instance).await?;
        for (name, value) in changes {
            let value = match value {
                RuleValue::Bool(on) => json!(on),
                RuleValue::Int(number) => json!(number),
            };
            client
                .call(
                    "minecraft:gamerules/update",
                    Some(json!({ "gamerule": { "key": name, "value": value } })),
                )
                .await
                .map_err(|error| match GameruleError::from(error) {
                    GameruleError::Refused(why) => GameruleError::Refused(format!("{name}: {why}")),
                    other => other,
                })?;
        }
        Ok(())
    }

    /// Types the commands, then has the server save, so its files show them.
    async fn through_console(
        &self,
        id: InstanceId,
        changes: &[(String, RuleValue)],
        author: &str,
    ) -> Result<(), GameruleError> {
        for (name, value) in changes {
            // The name came from the world's files, which the server writes: it must look like a name.
            if !is_plain_name(name) {
                return Err(GameruleError::UnknownRule(name.clone()));
            }
            self.instances
                .send_command(id, &format!("gamerule {name} {value}"), Some(author.to_owned()))
                .await?;
        }
        // Subscribed before asking, so the confirmation can't slip past.
        let mut events = self.events.subscribe();
        self.instances
            .send_command(id, SAVE_COMMAND, Some(author.to_owned()))
            .await?;
        wait_for_save(&mut events, id, SAVE_WAIT).await?;
        Ok(())
    }
}

/// A rule name that can go in a command as it is: letters, digits, `_`, `.`, `-`, and a namespace's `:`.
fn is_plain_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b':'))
}

async fn level_name(files: &ServerFiles) -> Result<String, GameruleError> {
    let path = SafeRelPath::parse("server.properties").map_err(FilesError::from)?;
    let text = match files.read_text(path).await {
        Ok(file) => file.content,
        Err(FilesError::NotFound(_)) => String::new(),
        Err(error) => return Err(error.into()),
    };
    Ok(worlds::level_name_in(&PropertiesDoc::parse(&text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_go_into_commands() {
        for good in [
            "keepInventory",
            "keep_inventory",
            "minecraft:keep_inventory",
            "somemod:rule.v2",
        ] {
            assert!(is_plain_name(good), "{good}");
        }
        for bad in ["", "a b", "x\nstop", "@a", "rule;stop", &"a".repeat(101)] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }
}
