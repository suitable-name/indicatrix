//! Which saved variant the open design came from, so a new variant can say "made from ...".
//!
//! Opening a variant makes it the design's branch; saving a variant makes the new one the
//! branch (so a run of saves reads as a chain, and opening an older variant and saving again
//! starts a second branch from it). The branch belongs to one design as it was opened: a
//! different design, or the same file opened again, has none.

/// The variant the open design was last opened from or saved as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Branch {
    /// The design's UUID, the design epoch it was recorded in, and the variant's id.
    current: Option<(String, u64, i64)>,
}

impl Branch {
    /// Records `variant` as the branch of the design `uuid` at design epoch `epoch`.
    pub(super) fn set(&mut self, uuid: &str, epoch: u64, variant: i64) {
        self.current = Some((uuid.to_owned(), epoch, variant));
    }

    /// The variant the design `uuid` (at design epoch `epoch`) came from, if any.
    pub(super) fn get(&self, uuid: &str, epoch: u64) -> Option<i64> {
        self.current
            .as_ref()
            .filter(|(branch_uuid, branch_epoch, _)| branch_uuid == uuid && *branch_epoch == epoch)
            .map(|&(_, _, variant)| variant)
    }

    /// Forgets the branch when it is `variant` (the variant was deleted).
    pub(super) fn forget(&mut self, variant: i64) {
        if self
            .current
            .as_ref()
            .is_some_and(|&(_, _, branch)| branch == variant)
        {
            self.current = None;
        }
    }
}
