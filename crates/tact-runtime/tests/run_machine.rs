use tact_contracts::{run::{RunId, RunOutcome},};
use tact_runtime::{RunMachine, RunState};

#[test]
fn run_machine_completes_once_and_reports_progress() {
    let mut machine = RunMachine::new(RunId(3));
    assert_eq!(machine.state(), RunState::Running);
    machine.record_model_turn();
    assert_eq!(machine.model_turns(), 1);
    assert_eq!(machine.complete(), Ok(RunOutcome::Completed));
    assert_eq!(machine.complete(), Err(tact_runtime::RunError::AlreadyTerminal));
}

#[test]
fn run_machine_cancellation_is_terminal() {
    let mut machine = RunMachine::new(RunId(4));
    assert_eq!(machine.cancel(), Ok(RunOutcome::Cancelled));
    assert_eq!(machine.state(), RunState::Cancelled);
    assert_eq!(machine.record_model_turn(), false);
}
