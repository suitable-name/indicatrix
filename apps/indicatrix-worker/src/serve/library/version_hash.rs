//! Computes `DesignSummary::version`/`DesignRecord::version`: a SHA-256 hash over
//! exactly the fields each response carries, in a fixed order, each length-prefixed so
//! adjacent fields can never collide, and each `Option` tagged present/absent before its
//! value. See the `serve::library` module doc comment for why this is computed here
//! rather than read from the database.

use indicatrix_net::library::{DesignRecord, DesignSummary};
use sha2::{Digest, Sha256};

fn hash_str(hasher: &mut Sha256, s: &str) {
    hasher.update((s.len() as u64).to_le_bytes());
    hasher.update(s.as_bytes());
}

fn hash_opt_str(hasher: &mut Sha256, s: Option<&str>) {
    match s {
        Some(s) => {
            hasher.update([1u8]);
            hash_str(hasher, s);
        }
        None => hasher.update([0u8]),
    }
}

fn hash_opt_bytes(hasher: &mut Sha256, b: Option<&[u8]>) {
    match b {
        Some(b) => {
            hasher.update([1u8]);
            hasher.update((b.len() as u64).to_le_bytes());
            hasher.update(b);
        }
        None => hasher.update([0u8]),
    }
}

/// [`hash_opt_str`]'s counterpart for [`DesignRecord::mirror_symmetry`]: same presence
/// tag before the payload byte, so `Some(false)` still hashes differently from `None`.
fn hash_opt_bool(hasher: &mut Sha256, b: Option<bool>) {
    match b {
        Some(b) => hasher.update([1u8, u8::from(b)]),
        None => hasher.update([0u8]),
    }
}

/// SHA-256 over every [`DesignSummary`] field except [`DesignSummary::version`] itself.
pub(super) fn hash_summary(s: &DesignSummary) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(s.entry_id.to_le_bytes());
    hash_str(&mut hasher, &s.title);
    hash_str(&mut hasher, &s.url);
    hash_opt_str(&mut hasher, s.design_id.as_deref());
    hash_opt_str(&mut hasher, s.shape.as_deref());
    hash_opt_str(&mut hasher, s.index_gear.as_deref());
    hash_opt_str(&mut hasher, s.facets_count.as_deref());
    hash_opt_str(&mut hasher, s.designer_info.as_deref());
    hash_opt_str(&mut hasher, s.lw_ratio.as_deref());
    hash_opt_str(&mut hasher, s.refractive_index.as_deref());
    hash_opt_str(&mut hasher, s.volume.as_deref());
    hash_opt_str(&mut hasher, s.competition_diagram.as_deref());
    hasher.update([u8::from(s.ignored)]);
    hasher.finalize().into()
}

/// SHA-256 over every [`DesignRecord`] field except [`DesignRecord::version`] itself --
/// including each attachment's metadata (id/name/url/size), never content.
pub(super) fn hash_record(r: &DesignRecord) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(r.entry_id.to_le_bytes());
    hash_str(&mut hasher, &r.title);
    hash_str(&mut hasher, &r.url);
    hash_opt_str(&mut hasher, r.design_id.as_deref());
    hash_str(&mut hasher, &r.page_url);
    hash_opt_str(&mut hasher, r.diagram_image_name.as_deref());
    hash_opt_bytes(&mut hasher, r.diagram_image_data.as_deref());
    hash_opt_str(&mut hasher, r.competition_diagram.as_deref());
    hash_opt_str(&mut hasher, r.lw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.refractive_index.as_deref());
    hash_opt_str(&mut hasher, r.index_gear.as_deref());
    hash_opt_str(&mut hasher, r.volume.as_deref());
    hash_opt_str(&mut hasher, r.facets_count.as_deref());
    hash_opt_str(&mut hasher, r.shape.as_deref());
    hash_opt_str(&mut hasher, r.designer_info.as_deref());
    hash_opt_str(&mut hasher, r.preview_material.as_deref());
    hash_opt_str(&mut hasher, r.hw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.tw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.uw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.pw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.cw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.symmetry_order.as_deref());
    hash_opt_bool(&mut hasher, r.mirror_symmetry);
    hash_opt_str(&mut hasher, r.designer.as_deref());
    hasher.update((r.angle_settings.len() as u64).to_le_bytes());
    for a in &r.angle_settings {
        hasher.update(a.order_index.to_le_bytes());
        hash_str(&mut hasher, &a.facet);
        hash_str(&mut hasher, &a.angle);
        hash_str(&mut hasher, &a.index);
        hash_str(&mut hasher, &a.notes);
    }
    hasher.update((r.attachments.len() as u64).to_le_bytes());
    for f in &r.attachments {
        hasher.update(f.id.to_le_bytes());
        hash_str(&mut hasher, &f.name);
        hash_str(&mut hasher, &f.url);
        hasher.update(f.size.to_le_bytes());
    }
    hasher.finalize().into()
}
