# Trajectory

Trajectory is the durable record of a Runtime execution. Event transport provides live delivery; the Trajectory recorder provides ordered facts for replay, recovery, debugging, audit, and evaluation.

Each `TrajectoryEvent` contains a trajectory ID, run ID, sequence number, timestamp, actor, event type, optional parent step, payload, and sensitivity. The in-memory recorder supplies ordered append and sequence queries for tests and embedded callers; `SqliteTrajectoryRecorder` persists the same model without changing the protocol shape.

Interactive and headless sessions subscribe the SQLite trajectory service to the shared `EventTransport` before the agent is started. Each run emits `RunStarted` with its stable run ID; streamed output, model and token status, interaction requests, completion, and cancellation share that event stream. Headless runs therefore persist replayable facts even though they have no terminal View.

The TUI projects Runtime events into its current widget state and sends start, cancel, and interaction responses as Runtime commands. Detailed tool-card lifecycle events and Tact-specific commands still use the in-process compatibility adapter while their neutral protocol forms are added.

Recorded event classes include run lifecycle, model calls, tool calls, permission decisions, interactions, plugin lifecycle, cancellation, timeout, compaction, recovery, and namespaced plugin facts. Host facts are emitted by Runtime services; plugins may append only namespaced custom facts.

Clients resume by requesting events from a sequence number. A View can therefore reconnect without treating its local message list as the source of truth.
