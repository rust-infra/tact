use std::{sync::Arc, thread};

use tact_agent_api::{AgentEvent, EventDelivery, EventPort, RunContext, RunId, SessionRef};

/// Event sink backed by a non-blocking callback worker.
pub struct CallbackEventPort<F> {
    sender: std::sync::Mutex<std::sync::mpsc::Sender<Arc<AgentEvent>>>,
    _worker: thread::JoinHandle<()>,
    _callback: std::marker::PhantomData<fn(F)>,
}

impl<F> CallbackEventPort<F>
where
    F: Fn(AgentEvent) + Send + 'static,
{
    pub fn new(callback: F) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<Arc<AgentEvent>>();
        let worker = thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                callback((*event).clone());
            }
        });
        Self {
            sender: std::sync::Mutex::new(sender),
            _worker: worker,
            _callback: std::marker::PhantomData,
        }
    }
}

impl<F> EventPort for CallbackEventPort<F>
where
    F: Fn(AgentEvent) + Send + 'static,
{
    fn emit(&self, event: AgentEvent) {
        self.emit_shared(Arc::new(event), EventDelivery::Terminal);
    }

    fn emit_shared(&self, event: Arc<AgentEvent>, _delivery: EventDelivery) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(event);
        }
    }

    fn emit_shared_with_context(
        &self,
        _context: &RunContext,
        event: Arc<AgentEvent>,
        delivery: EventDelivery,
    ) {
        self.emit_shared(event, delivery);
    }
}

/// Event sink backed by a context-aware, non-blocking callback worker.
pub struct ContextCallbackEventPort<F> {
    sender: std::sync::Mutex<std::sync::mpsc::Sender<(RunContext, Arc<AgentEvent>)>>,
    _worker: thread::JoinHandle<()>,
    _callback: std::marker::PhantomData<fn(F)>,
}

impl<F> ContextCallbackEventPort<F>
where
    F: Fn(RunContext, AgentEvent) + Send + 'static,
{
    pub fn new(callback: F) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<(RunContext, Arc<AgentEvent>)>();
        let worker = thread::spawn(move || {
            while let Ok((context, event)) = receiver.recv() {
                callback(context, (*event).clone());
            }
        });
        Self {
            sender: std::sync::Mutex::new(sender),
            _worker: worker,
            _callback: std::marker::PhantomData,
        }
    }
}

impl<F> EventPort for ContextCallbackEventPort<F>
where
    F: Fn(RunContext, AgentEvent) + Send + 'static,
{
    fn emit(&self, event: AgentEvent) {
        self.emit_shared_with_context(
            &empty_run_context(),
            Arc::new(event),
            EventDelivery::Terminal,
        );
    }

    fn emit_shared_with_context(
        &self,
        context: &RunContext,
        event: Arc<AgentEvent>,
        _delivery: EventDelivery,
    ) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send((context.clone(), event));
        }
    }
}

fn empty_run_context() -> RunContext {
    RunContext {
        session: SessionRef::new(""),
        run_id: RunId::new(""),
    }
}

/// Discard every runtime event.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopEventPort;

impl EventPort for NoopEventPort {
    fn emit(&self, _event: AgentEvent) {}

    fn emit_shared(&self, _event: Arc<AgentEvent>, _delivery: EventDelivery) {}

    fn emit_shared_with_context(
        &self,
        _context: &RunContext,
        _event: Arc<AgentEvent>,
        _delivery: EventDelivery,
    ) {
    }
}
