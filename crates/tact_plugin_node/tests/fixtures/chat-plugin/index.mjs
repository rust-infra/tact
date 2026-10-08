import readline from 'node:readline';

const pluginId = 'fixture.chat';
const protocol = { major: 1, minor: 0 };
const capabilities = [
  {
    name: 'chat.echo', kind: 'tool', version: '1', risk: 'read_only',
    input_schema: { type: 'object' }, output_schema: { type: 'object' },
  },
  {
    name: 'chat.start', kind: 'command', version: '1', risk: 'low',
    input_schema: { type: 'object' }, output_schema: { type: 'object' },
  },
  {
    name: 'chat.slow', kind: 'tool', version: '1', risk: 'read_only',
    input_schema: { type: 'object' }, output_schema: { type: 'object' },
  },
  {
    name: 'chat.malformed', kind: 'tool', version: '1', risk: 'read_only',
    input_schema: { type: 'object' },
  },
  {
    name: 'chat.crash', kind: 'tool', version: '1', risk: 'read_only',
    input_schema: { type: 'object' },
  },
];
let calls = 0;

function envelope(request, response) {
  return {
    protocol_version: protocol,
    request_id: request.request_id,
    plugin_id: pluginId,
    session_id: request.session_id,
    run_id: request.run_id,
    trajectory_id: request.trajectory_id,
    response,
  };
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    continue;
  }

  if (request.request?.type === 'handshake') {
    const mismatch = process.argv.includes('--mismatch');
    process.stdout.write(JSON.stringify(envelope(request, {
      type: 'handshake_accepted',
      protocol_version: mismatch ? { major: 2, minor: 0 } : protocol,
      features: ['capability_registration', 'events', 'cancel'],
    })) + '\n');
  } else if (request.request?.type === 'register') {
    const registered = process.argv.includes('--invalid')
      ? [...capabilities, { name: ' ', kind: 'tool', version: '1', risk: 'read_only' }]
      : capabilities;
    process.stdout.write(JSON.stringify(envelope(request, {
      type: 'registered', capabilities: registered,
    })) + '\n');
  } else if (request.request?.type === 'invoke') {
    const capability = request.request.capability;
    if (capability === 'chat.crash') process.exit(23);
    if (capability === 'chat.malformed') {
      process.stdout.write('{not-json}\n');
      continue;
    }
    if (capability === 'chat.slow') {
      await new Promise(resolve => setTimeout(resolve, 500));
    }
    calls += 1;
    process.stdout.write(JSON.stringify(envelope(request, {
      type: 'result', output: { ...request.request.input, calls, run_id: request.run_id },
    })) + '\n');
  } else if (request.request?.type === 'subscribe') {
    const event = request.request.from_sequence === 42
      ? {
          type: 'interaction_requested',
          request: {
            type: 'select', request_id: 'node-interaction',
            prompt: 'Choose a greeting', options: ['hello', 'hi'],
          },
        }
      : {
          type: 'text', run_id: null, role: 'assistant',
          content: `replay-from-${request.request.from_sequence ?? 0}`,
        };
    process.stdout.write(JSON.stringify(envelope(request, {
      type: 'event', event,
    })) + '\n');
  } else if (request.request?.type === 'interaction_response') {
    process.stdout.write(JSON.stringify(envelope(request, {
      type: 'result', output: { answered: request.request.response.type === 'selected' },
    })) + '\n');
  } else if (request.request?.type === 'shutdown') {
    process.stdout.write(JSON.stringify(envelope(request, {
      type: 'result', output: { stopped: true },
    })) + '\n');
    process.exit(0);
  }
}
