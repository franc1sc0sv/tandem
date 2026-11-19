//! The elevated task's ACL policy, without touching the system: given the owner and the
//! ACEs of a path, it says whether only administrators can write it. The real reading of the
//! DACL lives in `windows.rs`; here it is tested on any platform.

/// SYSTEM, Administrators and NT SERVICE\TrustedInstaller: the only ones that can write or own.
pub const TRUSTED_SIDS: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

const FILE_WRITE_DATA: u32 = 0x2; // and FILE_ADD_FILE on folders
const FILE_APPEND_DATA: u32 = 0x4; // y FILE_ADD_SUBDIRECTORY
const FILE_DELETE_CHILD: u32 = 0x40;
const DELETE: u32 = 0x1_0000;
const WRITE_DAC: u32 = 0x4_0000;
const WRITE_OWNER: u32 = 0x8_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Any of these permissions allows replacing what is there.
pub const WRITE_MASK: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE;

/// For strict ancestors what allows replacing or deleting an existing child is enough:
/// creating files or subfolders replaces nothing (`C:\` comes from the factory with
/// `Authenticated Users:(AD)`).
pub const ANCESTOR_MASK: u32 =
    FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL | GENERIC_WRITE;

/// How much rigor is applied to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rigor {
    /// The executable and its containing folder: creating a file there plants a DLL.
    Full,
    /// Folders higher up the path.
    Ancestor,
}

impl Rigor {
    pub fn mask(self) -> u32 {
        match self {
            Rigor::Full => WRITE_MASK,
            Rigor::Ancestor => ANCESTOR_MASK,
        }
    }
}

const ACCESS_ALLOWED: u8 = 0;
const ACCESS_ALLOWED_OBJECT: u8 = 5;
const ACCESS_ALLOWED_CALLBACK: u8 = 9;
const ACCESS_ALLOWED_CALLBACK_OBJECT: u8 = 11;
const INHERIT_ONLY_ACE: u8 = 0x08;

/// An allow ACE of the DACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub sid: String,
    pub mask: u32,
    /// It is only inherited by children: it does not apply to this object.
    pub inherit_only: bool,
}

/// What a raw ACE of the DACL contributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawAce {
    /// An allow ACE to take into account.
    Allow(Ace),
    /// Deny, audit, label...: it grants nothing, it does not count.
    Ignored,
}

/// Classifies an ACE by its type and flags. Object allow ACEs do not exist on files
/// and are not interpreted: `Err` (counted as insecure).
pub fn classify(ace_type: u8, flags: u8, mask: u32, sid: String) -> Result<RawAce, String> {
    match ace_type {
        ACCESS_ALLOWED | ACCESS_ALLOWED_CALLBACK => Ok(RawAce::Allow(Ace {
            sid,
            mask,
            inherit_only: flags & INHERIT_ONLY_ACE != 0,
        })),
        ACCESS_ALLOWED_OBJECT | ACCESS_ALLOWED_CALLBACK_OBJECT => {
            Err(format!("allow ACE with object (type {ace_type})"))
        }
        _ => Ok(RawAce::Ignored),
    }
}

/// Allowlist: the owner and every SID with write (according to `rigor`) must be trusted.
/// `Err` carries the reason.
pub fn verdict(owner: &str, aces: &[Ace], rigor: Rigor) -> Result<(), String> {
    let trusted = |sid: &str| TRUSTED_SIDS.contains(&sid);
    // The owner has implicit WRITE_DAC even if the DACL does not say so.
    if !trusted(owner) {
        return Err(format!("the owner is {owner}"));
    }
    for ace in aces {
        if !ace.inherit_only && ace.mask & rigor.mask() != 0 && !trusted(&ace.sid) {
            return Err(format!("{} can write (mask {:#x})", ace.sid, ace.mask));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ADMINS: &str = "S-1-5-32-544";
    const USER: &str = "S-1-5-21-1-2-3-1001";
    const READ: u32 = 0x12_00A9; // read and execute

    fn ace(sid: &str, mask: u32) -> Ace {
        Ace {
            sid: sid.into(),
            mask,
            inherit_only: false,
        }
    }

    #[test]
    fn only_trusted_pass() {
        let aces = [
            ace("S-1-5-18", GENERIC_ALL),
            ace(ADMINS, 0x1F_01FF),
            ace(TRUSTED_SIDS[2], GENERIC_ALL),
            ace("S-1-5-32-545", READ),
        ];
        assert_eq!(verdict(ADMINS, &aces, Rigor::Full), Ok(()));
    }

    #[test]
    fn foreign_owner_is_rejected_with_a_clean_dacl() {
        let aces = [ace(ADMINS, GENERIC_ALL), ace(USER, READ)];
        assert!(
            verdict(USER, &aces, Rigor::Full)
                .unwrap_err()
                .contains("owner")
        );
    }

    #[test]
    fn any_write_permission_of_an_unlisted_sid_is_rejected() {
        for sid in ["S-1-5-4", "S-1-5-32-547", USER, "S-1-3-0", "S-1-1-0"] {
            for bit in [
                FILE_WRITE_DATA,
                FILE_APPEND_DATA,
                FILE_DELETE_CHILD,
                DELETE,
                WRITE_DAC,
                WRITE_OWNER,
                GENERIC_WRITE,
                GENERIC_ALL,
            ] {
                let aces = [ace(ADMINS, GENERIC_ALL), ace(sid, READ | bit)];
                assert!(
                    verdict(ADMINS, &aces, Rigor::Full).is_err(),
                    "{sid} {bit:#x}"
                );
            }
        }
    }

    #[test]
    fn read_only_for_other_sids_passes() {
        assert_eq!(verdict(ADMINS, &[ace(USER, READ)], Rigor::Full), Ok(()));
    }

    #[test]
    fn the_inherit_only_ace_does_not_apply_to_the_object() {
        let heredable = Ace {
            inherit_only: true,
            ..ace("S-1-3-0", GENERIC_ALL)
        };
        assert_eq!(verdict(ADMINS, &[heredable], Rigor::Full), Ok(()));
    }

    #[test]
    fn classifies_by_type_and_flags() {
        let sid = || USER.to_string();
        assert_eq!(classify(0, 0, 2, sid()), Ok(RawAce::Allow(ace(USER, 2))));
        let Ok(RawAce::Allow(a)) = classify(9, 0x08 | 0x01, 2, sid()) else {
            panic!("callback");
        };
        assert!(a.inherit_only);
        assert_eq!(classify(1, 0, GENERIC_ALL, sid()), Ok(RawAce::Ignored));
        assert_eq!(classify(2, 0, GENERIC_ALL, sid()), Ok(RawAce::Ignored));
        assert!(classify(5, 0, 2, sid()).is_err());
        assert!(classify(11, 0, 2, sid()).is_err());
    }

    #[test]
    fn factory_acl_of_the_root_passes_as_ancestor_and_fails_as_container() {
        // `icacls C:\\` of a Windows 11: Authenticated Users has (OI)(CI)(IO)(M), which does
        // not apply to the root, and (AD) without inheritance.
        const AUTH: &str = "S-1-5-11";
        const USERS: &str = "S-1-5-32-545";
        let inherit_only = |sid: &str, mask| Ace {
            inherit_only: true,
            ..ace(sid, mask)
        };
        let aces = [
            ace(ADMINS, 0x1F_01FF),
            ace("S-1-5-18", 0x1F_01FF),
            ace(USERS, READ),
            inherit_only(AUTH, 0x1301BF),
            ace(AUTH, FILE_APPEND_DATA),
        ];
        assert_eq!(
            verdict(
                "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
                &aces,
                Rigor::Ancestor
            ),
            Ok(())
        );
        assert!(
            verdict(ADMINS, &aces, Rigor::Full)
                .unwrap_err()
                .contains(AUTH)
        );
        // Creating subfolders is not enough as an ancestor, but deleting children is.
        let deletes = [ace(AUTH, FILE_DELETE_CHILD)];
        assert!(verdict(ADMINS, &deletes, Rigor::Ancestor).is_err());
    }

    proptest! {
        // With a trusted owner, the verdict is exactly "no applicable ACE of a
        // foreign SID with write access".
        #[test]
        fn verdict_equals_the_model(
            raw in proptest::collection::vec((0usize..5, any::<u32>(), any::<bool>()), 0..8)
        ) {
            let sids = [ADMINS, "S-1-5-18", USER, "S-1-5-4", "S-1-1-0"];
            let aces: Vec<Ace> = raw.iter().map(|&(i, mask, inherit_only)| Ace {
                sid: sids[i].into(), mask, inherit_only,
            }).collect();
            let dangerous = aces.iter().any(|a| {
                !a.inherit_only && a.mask & Rigor::Full.mask() != 0 && !TRUSTED_SIDS.contains(&a.sid.as_str())
            });
            prop_assert_eq!(verdict(ADMINS, &aces, Rigor::Full).is_err(), dangerous);
        }
    }
}
