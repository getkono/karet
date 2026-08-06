use super::*;

pub(super) fn run_merge_conflict(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    path: PathBuf,
    cancel: &Cancellation,
) {
    if cancel.is_cancelled() {
        return;
    }
    let result = repository(root).and_then(|repo| {
        let sides = repo
            .conflict_sides(&path)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{} is no longer conflicted", path.display()))?;
        let current = String::from_utf8(sides.current)
            .map_err(|_| format!("{} current side is not UTF-8 text", path.display()))?;
        let incoming = String::from_utf8(sides.incoming)
            .map_err(|_| format!("{} incoming side is not UTF-8 text", path.display()))?;
        Ok((current, incoming))
    });
    match result {
        Ok((current, incoming)) => emit_cancellable(
            events,
            id,
            cancel,
            Event::MergeConflictReady {
                path,
                current,
                incoming,
            },
        ),
        Err(message) => notify_cancellable(events, id, cancel, message),
    }
}
