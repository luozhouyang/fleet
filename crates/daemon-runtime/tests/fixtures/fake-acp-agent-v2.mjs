// Minimal ACP v2 (draft) agent used by the generic ACP driver tests.
//
// Newline-delimited JSON-RPC over stdio. `--permission` makes the prompt
// request a permission before the idle state update.

import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
const send = (message) => process.stdout.write(JSON.stringify(message) + '\n');
const permissionMode = process.argv.includes('--permission');
const mcpMode = process.argv.includes('--mcp');

let pendingPrompt = null;
let mcpCount = 0;

function update(update) {
  send({
    jsonrpc: '2.0',
    method: 'session/update',
    params: { sessionId: 'sess_v2_1', update },
  });
}

rl.on('line', (line) => {
  if (!line.trim()) {
    return;
  }
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method } = message;

  if (method === 'initialize') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: 2,
        info: { name: 'fake-v2-agent', version: '1.0.0', title: 'Fake ACP v2' },
        capabilities: {
          session: {
            prompt: { image: {}, embeddedContext: {} },
          },
        },
      },
    });
    return;
  }

  if (method === 'session/new') {
    mcpCount = Array.isArray(message.params?.mcpServers) ? message.params.mcpServers.length : 0;
    send({ jsonrpc: '2.0', id, result: { sessionId: 'sess_v2_1' } });
    return;
  }

  if (method === 'session/prompt') {
    pendingPrompt = id;

    // Echo the user message before responding. This exercises prompt
    // correlation: the driver must not render a duplicate user message.
    update({
      sessionUpdate: 'user_message',
      messageId: 'native_user_1',
      content: [{ type: 'text', text: 'hello' }],
    });

    send({ jsonrpc: '2.0', id, result: { messageId: 'native_user_1' } });

    update({ sessionUpdate: 'state_update', state: 'running' });

    if (process.argv.includes('--crash')) {
      update({
        sessionUpdate: 'agent_message_chunk',
        messageId: 'agent_msg_crash',
        content: { type: 'text', text: 'partial' },
      });
      // Simulate the agent process dying mid-turn.
      process.exit(3);
    }

    update({
      sessionUpdate: 'agent_message',
      messageId: 'agent_msg_1',
      content: [{ type: 'text', text: mcpMode ? `Hello v2 mcp=${mcpCount}` : 'Hello ' }],
    });
    update({
      sessionUpdate: 'agent_message_chunk',
      messageId: 'agent_msg_1',
      content: { type: 'text', text: mcpMode ? `v2 mcp=${mcpCount}` : 'v2' },
    });

    update({
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tool_v2',
      name: 'grep',
      title: 'Search files',
      status: 'in_progress',
      rawInput: { pattern: 'needle' },
    });
    update({
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tool_v2',
      status: 'completed',
      rawOutput: { matches: 2 },
    });

    update({
      sessionUpdate: 'plan_update',
      plan: {
        type: 'items',
        planId: 'plan_v2',
        entries: [
          { content: 'Search the tree', priority: 'high', status: 'completed' },
          { content: 'Report results', priority: 'medium', status: 'in_progress' },
        ],
      },
    });

    update({
      sessionUpdate: 'terminal_update',
      terminalId: 'term_v2',
      command: 'cargo test',
      cwd: '/tmp',
      output: { data: Buffer.from('ok\n').toString('base64') },
      exitStatus: { exitCode: 0 },
    });
    update({
      sessionUpdate: 'terminal_output_chunk',
      terminalId: 'term_v2',
      data: Buffer.from('more').toString('base64'),
    });

    if (permissionMode) {
      send({
        jsonrpc: '2.0',
        id: 900,
        method: 'session/request_permission',
        params: {
          sessionId: 'sess_v2_1',
          title: 'Run tests',
          description: 'The agent wants to run the test suite',
          subject: {
            type: 'tool_call',
            toolCall: {
              toolCallId: 'tool_v2',
              name: 'grep',
              title: 'Search files',
              status: 'in_progress',
            },
          },
          options: [
            { optionId: 'allow_once', name: 'Allow once', kind: 'allow_once' },
            { optionId: 'reject_once', name: 'Reject', kind: 'reject_once' },
          ],
        },
      });
    } else {
      finishTurn();
    }
    return;
  }

  if (method === 'session/cancel') {
    if (pendingPrompt !== null) {
      pendingPrompt = null;
      update({
        sessionUpdate: 'state_update',
        state: 'idle',
        stopReason: 'cancelled',
      });
    }
    return;
  }

  // Response to our permission request.
  if (id === 900) {
    finishTurn();
  }
});

function finishTurn() {
  update({
    sessionUpdate: 'state_update',
    state: 'idle',
    stopReason: 'end_turn',
  });
  pendingPrompt = null;
}
