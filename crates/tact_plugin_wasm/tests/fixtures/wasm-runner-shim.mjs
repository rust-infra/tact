import readline from 'node:readline';

const pluginId = 'fixture.wasm';
const protocol = { major: 1, minor: 0 };
const capabilities = [
  { name: 'wasm.echo', kind: 'tool', version: '1', risk: 'read_only', input_schema: { type: 'object' } },
  { name: 'wasm.slow', kind: 'tool', version: '1', risk: 'read_only', input_schema: { type: 'object' } },
  { name: 'wasm.self', kind: 'tool', version: '1', risk: 'read_only', input_schema: { type: 'object' } },
];
const runtimeArgs = process.argv.slice(2);
let pendingInput = {};
let pendingRunId = null;

function respond(request, response) {
  process.stdout.write(JSON.stringify({
    protocol_version: protocol,
    request_id: request.request_id,
    plugin_id: pluginId,
    session_id: request.session_id,
    run_id: request.run_id,
    trajectory_id: request.trajectory_id,
    response,
  }) + '\n');
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  const request = JSON.parse(line);
  switch (request.request.type) {
    case 'handshake':
      respond(request, { type: 'handshake_accepted', protocol_version: protocol, features: ['capability_registration', 'events', 'cancel', 'host_calls'] });
      break;
    case 'register':
      respond(request, { type: 'registered', capabilities });
      break;
    case 'invoke':
      if (request.request.capability === 'wasm.slow') {
        await new Promise(resolve => setTimeout(resolve, 500));
      }
      if (request.request.capability === 'wasm.echo') {
        pendingInput = request.request.input;
        pendingRunId = request.run_id;
        respond(request, {
          type: 'host_call', host_request_id: 'wasm-clock-1',
          capability: 'clock.read', input: null,
        });
        break;
      }
      if (request.request.capability === 'wasm.self') {
        respond(request, {
          type: 'host_call', host_request_id: 'wasm-self-1',
          capability: 'capability:wasm.self', input: {},
        });
        break;
      }
      respond(request, { type: 'result', output: { ...request.request.input, runner_args: runtimeArgs } });
      break;
    case 'host_call_result':
      respond(request, {
        type: 'result',
        output: {
          ...pendingInput,
          clock: request.request.output,
          denied: request.request.error !== null,
          run_id: pendingRunId,
          runner_args: runtimeArgs,
        },
      });
      break;
    case 'shutdown':
      respond(request, { type: 'result', output: { dropped: true } });
      process.exit(0);
  }
}
