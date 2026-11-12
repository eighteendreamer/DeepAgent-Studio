//! Approved native transcript views for the existing external Hook boundary.
//! These files are derived exports, never read back as session authority.
use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{Hook, HookBaseInput, HookContext, HookOutcome, HookReport};
use deepagent_observation::{export_transcript, TranscriptFormat};
use deepagent_persistence::{event_store::EventStore, Database};
use deepagent_tools::ToolExecutionContext;

const FORMAT: &str = "deepagent.events.v1";

#[cfg(test)]
#[path = "hook_input_tests.rs"]
mod tests;

struct NativeView {
    file: File,
    path: tempfile::TempPath,
    original_permissions: Permissions,
}

impl Drop for NativeView {
    fn drop(&mut self) {
        // Windows cannot unlink read-only files. Restore the original bits on
        // our open handle, not by following a caller-replaced path.
        if let Err(error) = self.file.set_permissions(self.original_permissions.clone()) {
            tracing::warn!(%error, "could not release native hook view permissions");
        }
    }
}

/// One run's shared input boundary. Each action leases its own snapshot, so
/// removed/changed exports are never reused and concurrent callbacks cannot
/// replace one another's input. No derived history survives normal dispatch.
pub(crate) struct HookInputScope {
    db: Arc<Database>,
    root: PathBuf,
}

impl HookInputScope {
    pub(crate) fn new(db: Arc<Database>, root: PathBuf) -> Arc<Self> {
        Arc::new(Self { db, root })
    }

    pub(crate) fn wrap(self: &Arc<Self>, inner: Arc<dyn Hook>) -> Arc<dyn Hook> {
        Arc::new(ScopedHook {
            scope: self.clone(),
            inner,
        })
    }

    fn prepare(&self, ctx: &HookContext) -> Result<(HookBaseInput, NativeView)> {
        let root = std::fs::canonicalize(&self.root)
            .map_err(|error| CoreError::Persistence(format!("hook workspace: {error}")))?;
        let store = EventStore::new(&self.db);
        let record = store
            .get_session(ctx.session_id)?
            .ok_or_else(|| CoreError::not_found("hook input session is missing"))?;
        if let Some(project) = &record.project {
            if std::fs::canonicalize(project).map_err(|error| {
                CoreError::Persistence(format!("hook session workspace: {error}"))
            })? != root
            {
                return Err(CoreError::invalid(
                    "hook input session belongs to a different workspace",
                ));
            }
        }
        let events = store.load_session(ctx.session_id)?;
        if events.is_empty() {
            return Err(CoreError::invalid("hook transcript is empty"));
        }
        // No database/session lock crosses the external action await.
        let content = export_transcript(record.title.as_deref(), &events, TranscriptFormat::Json)
            .map_err(|error| {
            CoreError::other(format!("native hook transcript export: {error}"))
        })?;
        // The storage owner supplies the layout. Never hand out the
        // compressed authority or a path supplied by hook output.
        let directory = std::fs::canonicalize(self.db.session_transcript_dir(ctx.session_id))
            .map_err(|error| CoreError::Persistence(format!("hook view directory: {error}")))?;
        let mut file = tempfile::Builder::new()
            .prefix(".hook-transcript-v1-")
            .suffix(".json")
            .tempfile_in(&directory)
            .map_err(|error| CoreError::Persistence(format!("create hook view: {error}")))?;
        file.write_all(content.as_bytes())
            .and_then(|_| file.flush())
            .map_err(|error| CoreError::Persistence(format!("write hook view: {error}")))?;
        let original_permissions = file
            .as_file()
            .metadata()
            .map_err(|error| CoreError::Persistence(format!("hook view metadata: {error}")))?
            .permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        // A live writable handle makes .NET File.ReadAllText fail Windows
        // sharing checks even after setting the read-only attribute. Retain
        // only read/attribute access while the external callback is running.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_GENERIC_READ, FILE_WRITE_ATTRIBUTES,
            };
            options.access_mode(FILE_GENERIC_READ | FILE_WRITE_ATTRIBUTES);
        }
        let reader = options
            .open(file.path())
            .map_err(|error| CoreError::Persistence(format!("open hook view reader: {error}")))?;
        let (writer, path) = file.into_parts();
        drop(writer);
        let view = NativeView {
            file: reader,
            path,
            original_permissions,
        };
        view.file
            .set_permissions(readonly)
            .map_err(|error| CoreError::Persistence(format!("protect hook view: {error}")))?;
        let input = HookBaseInput {
            cwd: root
                .to_str()
                .ok_or_else(|| CoreError::invalid("hook workspace is not UTF-8"))?
                .into(),
            transcript_path: view
                .path
                .to_str()
                .ok_or_else(|| CoreError::invalid("hook transcript path is not UTF-8"))?
                .into(),
            transcript_format: FORMAT.into(),
        };
        Ok((input, view))
    }
}

struct ScopedHook {
    scope: Arc<HookInputScope>,
    inner: Arc<dyn Hook>,
}

#[async_trait]
impl Hook for ScopedHook {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dedup_key(&self) -> String {
        format!(
            "{}:input_scope:{:p}",
            self.inner.dedup_key(),
            Arc::as_ptr(&self.scope)
        )
    }

    fn applies(&self, ctx: &HookContext) -> bool {
        self.inner.applies(ctx)
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        self.run_with_context(ctx, ToolExecutionContext::default())
            .await
    }

    async fn run_with_context(
        &self,
        ctx: &HookContext,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        self.run_report(ctx, execution)
            .await
            .map(|report| report.outcome)
    }

    async fn run_report(
        &self,
        ctx: &HookContext,
        execution: ToolExecutionContext,
    ) -> Result<HookReport> {
        if !self.inner.applies(ctx) {
            return Ok(HookReport::decision(HookOutcome::Continue));
        }
        if execution.is_cancelled() {
            return Err(CoreError::other("hook input cancelled"));
        }
        let (input, _lease) = self.scope.prepare(ctx)?;
        self.inner
            .run_report(&ctx.clone().with_base_input(input), execution)
            .await
    }
}

pub(crate) fn wrap_input(
    scope: Option<&Arc<HookInputScope>>,
    hook: Arc<dyn Hook>,
) -> Arc<dyn Hook> {
    match scope {
        Some(scope) => scope.wrap(hook),
        None => hook,
    }
}
