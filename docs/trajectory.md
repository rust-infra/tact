# Trajectory

Trajectory is the durable record of a Runtime execution. Event transport provides live delivery; the Trajectory recorder provides ordered facts for replay, recovery, debugging, audit, and evaluation.

Each `TrajectoryEvent` contains a trajectory ID, run ID, sequence number, timestamp, actor, event type, optional parent step, payload, and sensitivity. The in-memory recorder supplies ordered append and sequence queries for tests and embedded callers; `SqliteTrajectoryRecorder` persists the same model without changing the protocol shape.

Interactive sessions subscribe the SQLite trajectory service to the shared `EventTransport` before the agent is started. The TUI also subscribes to that transport for plugin lifecycle and namespaced plugin events, while the legacy `AgentUpdate` channel remains available for the existing rich widgets and prompt flows. This makes the Runtime event stream the durable and cross-view boundary without dropping current business behavior.

Recorded event classes include run lifecycle, model calls, tool calls, permission decisions, interactions, plugin lifecycle, cancellation, timeout, compaction, recovery, and namespaced plugin facts. Host facts are emitted by Runtime services; plugins may append only namespaced custom facts.

Clients resume by requesting events from a sequence number. A View can therefore reconnect without treating its local message list as the source of truth.
