//! Path-compatibility shim: [`SampleCursor`] moved to `bridge::sample_cursor` so the
//! live viewport's hybrid path can share it. Re-exported here so every export-side
//! `super::sample_cursor::SampleCursor` import keeps compiling unchanged.

pub(in crate::bridge::export_thread) use crate::bridge::sample_cursor::SampleCursor;
