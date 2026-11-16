// Minimal ACP v1 agent used by the provider integration tests.
//
// It speaks newline-delimited JSON-RPC over stdio. Pass `--question` to make
// `session/prompt` raise a form elicitation instead of a permission request.

import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
const send = (message) => process.stdout.write(JSON.stringify(message) + '\n');
const questionMode =
  process.argv.includes('--question') || process.env.FAKE_ACP_MODE === 'question';

let pendingPromptId = null;

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
        protocolVersion: 1,
        agentCapabilities: {
          promptCapabilities: { image: false, audio: false, embeddedContext: false },
          sessionCapabilities: {},
        },
      },
    });
    return;
  }

  if (method === 'session/new') {
    send({ jsonrpc: '2.0', id, result: { sessionId: 'sess_acp_1' } });
    return;
  }

  if (method === 'session/prompt') {
    pendingPromptId = id;
    send({
      jsonrpc: '2.0',
      method: 'session/update',
      params: {
        sessionId: 'sess_acp_1',
        update: {
          sessionUpdate: 'agent_message_chunk',
          messageId: 'msg_1',
          content: { type: 'text', text: 'Hello from ACP' },
        },
      },
    });

    if (questionMode) {
      send({
        jsonrpc: '2.0',
        id: 901,
        method: 'elicitation/create',
        params: {
          mode: 'form',
          sessionId: 'sess_acp_1',
          message: 'Please answer',
          requestedSchema: {
            type: 'object',
            properties: {
              question_0: {
                type: 'string',
                title: 'Deploy',
                description: 'Which environment?',
                oneOf: [
                  { const: 'staging', title: 'staging', description: 'Safe' },
                  { const: 'production', title: 'production', description: 'Live' },
                ],
              },
              question_0_custom: { type: 'string', title: 'Other' },
            },
          },
        },
      });
    } else {
      send({
        jsonrpc: '2.0',
        id: 900,
        method: 'session/request_permission',
        params: {
          sessionId: 'sess_acp_1',
          toolCall: { toolCallId: 'tool_1', title: 'Run tests', status: 'pending' },
          options: [
            { optionId: 'allow_once', name: 'Allow once', kind: 'allow_once' },
            { optionId: 'reject_once', name: 'Reject', kind: 'reject_once' },
          ],
        },
      });
    }
    return;
  }

  if (method === 'session/cancel') {
    if (pendingPromptId !== null) {
      send({
        jsonrpc: '2.0',
        id: pendingPromptId,
        result: { stopReason: 'cancelled' },
      });
      pendingPromptId = null;
    }
    return;
  }

  // Responses to our own requests (permission 900, elicitation 901).
  if (id === 900 || id === 901) {
    if (pendingPromptId !== null) {
      send({
        jsonrpc: '2.0',
        id: pendingPromptId,
        result: { stopReason: 'end_turn' },
      });
      pendingPromptId = null;
    }
  }
});

rl.on('close', () => process.exit(0));
