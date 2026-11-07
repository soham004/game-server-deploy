//! The authenticated user and their effective permissions.

use std::collections::HashMap;

use crate::{
    auth::{GlobalPermission, GlobalRole, Permission, PermissionSet, User},
    ids::{InstanceId, UserId},
};

/// Who is performing an action. Built once per request by [`crate::auth::AuthService::authenticate`]
/// and passed to services so they can authorise and audit.
#[derive(Debug, Clone)]
pub struct Actor {
    user: User,
    grants: HashMap<InstanceId, PermissionSet>,
}

impl Actor {
    pub fn new(user: User, grants: HashMap<InstanceId, PermissionSet>) -> Self {
        Self { user, grants }
    }

    pub fn user(&self) -> &User {
        &self.user
    }

    pub fn id(&self) -> UserId {
        self.user.id
    }

    pub fn role(&self) -> GlobalRole {
        self.user.role
    }

    /// Effective permissions on one instance.
    pub fn permissions_for(&self, instance: InstanceId) -> PermissionSet {
        if self.user.role.can_access_all_instances() {
            PermissionSet::all()
        } else {
            self.grants.get(&instance).copied().unwrap_or_default()
        }
    }

    pub fn can(&self, permission: Permission, instance: InstanceId) -> bool {
        self.permissions_for(instance).contains(permission)
    }

    pub fn can_global(&self, permission: GlobalPermission) -> bool {
        self.user.role.global_permissions().contains(&permission)
    }

    /// Explicit per-instance grants (empty for owners and admins, who have implicit access).
    pub fn instance_grants(&self) -> impl Iterator<Item = (InstanceId, PermissionSet)> + '_ {
        self.grants.iter().map(|(id, set)| (*id, *set))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::auth::{Grant, Username};

    fn user(role: GlobalRole) -> User {
        User {
            id: UserId::generate(),
            username: Username::parse("tester").unwrap(),
            role,
            must_change_password: false,
            disabled: false,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn admins_can_do_everything_on_any_instance() {
        let actor = Actor::new(user(GlobalRole::Admin), HashMap::new());
        assert!(actor.can(Permission::FilesWrite, InstanceId::generate()));
        assert!(actor.can_global(GlobalPermission::InstanceCreate));
        assert!(!actor.can_global(GlobalPermission::UsersManage));
    }

    #[test]
    fn members_only_get_what_they_were_granted() {
        let granted = InstanceId::generate();
        let other = InstanceId::generate();
        let grants = HashMap::from([(granted, Grant::Operator.permissions())]);
        let actor = Actor::new(user(GlobalRole::Member), grants);

        assert!(actor.can(Permission::ConsoleWrite, granted));
        assert!(!actor.can(Permission::FilesRead, granted));
        assert!(!actor.can(Permission::InstanceView, other));
        assert!(!actor.can_global(GlobalPermission::InstanceCreate));
    }
}
