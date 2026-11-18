use crate::AnalysisStore;
use nexora_platform::PlatformError;
use postgres::Transaction;
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) struct AnalysisProfileReportStore;
impl AnalysisProfileReportStore {
    fn invalid() -> PlatformError {
        PlatformError::Validation("invalid analysis profile selection report")
    }
    fn fields(value: &Value, fields: &[&str]) -> Result<(), PlatformError> {
        let object = value.as_object().ok_or_else(Self::invalid)?;
        if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
            return Err(Self::invalid());
        }
        Ok(())
    }
    fn checksum(value: &Value) -> Result<(), PlatformError> {
        if value.as_str().is_none_or(|s| {
            s.len() != 64
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(Self::invalid());
        }
        Ok(())
    }
    fn paths(value: &Value, all: &mut BTreeSet<String>) -> Result<(), PlatformError> {
        let paths = value.as_array().ok_or_else(Self::invalid)?;
        for path in paths {
            let path = path.as_str().ok_or_else(Self::invalid)?;
            if path.is_empty()
                || path.len() > 4096
                || path.starts_with('/')
                || path.contains('\\')
                || path.contains('\0')
                || path.split('/').any(|part| part == ".." || part == ".")
                || !all.insert(path.into())
            {
                return Err(Self::invalid());
            }
        }
        Ok(())
    }
    pub fn validate(
        tx: &mut Transaction<'_>,
        plan: &Value,
        selection: &Value,
    ) -> Result<(), PlatformError> {
        if plan.is_null() {
            return Err(Self::invalid());
        }
        let mut fields = vec!["policy", "profiles", "uncovered_files", "excluded_files"];
        if selection.get("excluded_directories").is_some() {
            fields.push("excluded_directories");
        }
        let source_metadata = selection.get("status").is_some()
            || selection.get("skipped_files").is_some()
            || selection.get("decoded_files").is_some();
        if source_metadata {
            fields.extend(["status", "skipped_files", "decoded_files"]);
        }
        Self::fields(selection, &fields)?;
        if source_metadata && !matches!(selection["status"].as_str(), Some("complete" | "partial"))
        {
            return Err(Self::invalid());
        }
        if selection["policy"] != plan["policy"] {
            return Err(Self::invalid());
        }
        let profiles = selection["profiles"].as_array().ok_or_else(Self::invalid)?;
        if (profiles.is_empty() && !source_metadata) || profiles.len() > 1024 {
            return Err(Self::invalid());
        }
        let pinned = plan["profile_version_ids"]
            .as_array()
            .ok_or_else(Self::invalid)?;
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let skipped = if source_metadata {
            selection["skipped_files"]
                .as_array()
                .ok_or_else(Self::invalid)?
                .as_slice()
        } else {
            &[]
        };
        if !skipped.is_empty() && selection["status"] != "partial" {
            return Err(Self::invalid());
        }
        let mut skipped_profiles = BTreeSet::new();
        for file in skipped {
            if file.get("checksum").is_some() {
                Self::fields(file, &["path", "profile_id", "reason", "checksum"])?;
                Self::checksum(&file["checksum"])?;
            } else {
                // A failed file read has no content from which to compute a checksum.
                Self::fields(file, &["path", "profile_id", "reason"])?;
            }
            let id = file["profile_id"].as_str().ok_or_else(Self::invalid)?;
            if !pinned.iter().any(|v| v.as_str() == Some(id))
                || file["reason"].as_str().is_none_or(|s| {
                    s.is_empty() || s.len() > 512 || s.chars().any(char::is_control)
                })
            {
                return Err(Self::invalid());
            }
            Self::paths(&serde_json::json!([file["path"]]), &mut paths)?;
            skipped_profiles.insert(id);
        }
        for profile in profiles {
            Self::fields(
                profile,
                &[
                    "id",
                    "key",
                    "version",
                    "name",
                    "language_id",
                    "checksum",
                    "files",
                ],
            )?;
            let id = profile["id"].as_str().ok_or_else(Self::invalid)?;
            if !pinned.iter().any(|v| v.as_str() == Some(id)) || !ids.insert(id) {
                return Err(Self::invalid());
            }
            let row=tx.query_opt("SELECT to_jsonb(p)-'package'-'created_by'-'created_at' FROM analysis_profile_versions p WHERE id=$1",&[&id]).map_err(AnalysisStore::database_error)?.ok_or_else(Self::invalid)?;
            let summary: Value = row.get(0);
            for field in ["id", "key", "version", "name", "language_id", "checksum"] {
                if profile[field] != summary[field] {
                    return Err(Self::invalid());
                }
            }
            if profile["files"]
                .as_array()
                .is_none_or(|files| files.is_empty() && !skipped_profiles.contains(id))
            {
                return Err(Self::invalid());
            }
            Self::paths(&profile["files"], &mut paths)?;
        }
        if skipped_profiles.iter().any(|id| !ids.contains(id)) {
            return Err(Self::invalid());
        }
        if source_metadata {
            let decoded = selection["decoded_files"]
                .as_array()
                .ok_or_else(Self::invalid)?;
            let mut decoded_paths = BTreeSet::new();
            for file in decoded {
                Self::fields(file, &["path", "encoding", "checksum"])?;
                Self::checksum(&file["checksum"])?;
                Self::paths(&serde_json::json!([file["path"]]), &mut decoded_paths)?;
                if !matches!(
                    file["encoding"].as_str(),
                    Some("UTF-8-BOM" | "UTF-16LE" | "UTF-16BE")
                ) || !profiles.iter().any(|p| {
                    p["files"]
                        .as_array()
                        .is_some_and(|files| files.contains(&file["path"]))
                }) {
                    return Err(Self::invalid());
                }
            }
        }
        Self::paths(&selection["uncovered_files"], &mut paths)?;
        Self::paths(&selection["excluded_files"], &mut paths)?;
        if let Some(directories) = selection.get("excluded_directories") {
            Self::paths(directories, &mut paths)?;
        }
        Ok(())
    }
}
