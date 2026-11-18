// Minimal ACP v1 agent used by the provider integration tests.
//
// It speaks newline-delimited JSON-RPC over stdio. Pass `--question` to make
// `session/prompt` raise a form elicitation instead of a permission request.

import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
const send = (message) => process.stdout.write(JSON.stringify(message) + '\n');
const questionMode =
  process.argv.includes('--question') || process.env.FAKE_ACP_MODE === 'question';
const fsRootArg = process.argv.find((arg) => arg.startsWith('--fs-root='));
const fsRoot = fsRootArg ? fsRootArg.slice('--fs-root='.length) : null;
const fsEscapeMode = process.argv.includes('--fs-escape');
const resumeMode = process.argv.includes('--resume');
const crashMode = process.argv.includes('--crash');
const terminalMode = process.argv.includes('--terminal');

let pendingPromptId = null;
const clientFs = { read: false, write: false };
const clientTerminal = { enabled: false };
let terminalId = null;
let terminalOutput = '';

function finishPrompt(text) {
  if (text === 'fs done') {
    text = `fs done advertised=${clientFs.read && clientFs.write}`;
  }
  send({
    jsonrpc: '2.0',
    method: 'session/update',
    params: {
      sessionId: 'sess_acp_1',
      update: {
        sessionUpdate: 'agent_message_chunk',
        messageId: 'msg_1',
        content: { type: 'text', text },
      },
    },
  });
  if (pendingPromptId !== null) {
    send({
      jsonrpc: '2.0',
      id: pendingPromptId,
      result: { stopReason: 'end_turn' },
    });
    pendingPromptId = null;
  }
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
    const fs = message.params?.clientCapabilities?.fs;
    clientFs.read = Boolean(fs?.readTextFile);
    clientFs.write = Boolean(fs?.writeTextFile);
    clientTerminal.enabled = Boolean(message.params?.clientCapabilities?.terminal);
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: 1,
        agentCapabilities: {
          promptCapabilities: { image: false, audio: false, embeddedContext: false },
          sessionCapabilities: resumeMode ? { resume: {} } : {},
        },
      },
    });
    return;
  }

  if (method === 'session/new') {
    send({ jsonrpc: '2.0', id, result: { sessionId: 'sess_acp_1' } });
    return;
  }

  if (method === 'session/resume') {
    send({ jsonrpc: '2.0', id, result: {} });
    return;
  }

  if (method === 'session/prompt') {
    pendingPromptId = id;
    if (crashMode && JSON.stringify(message.params?.prompt ?? '').includes('crash')) {
      process.exit(1);
    }
    if (terminalMode) {
      send({
        jsonrpc: '2.0',
        id: 904,
        method: 'terminal/create',
        params: {
          sessionId: 'sess_acp_1',
          command: 'sh',
          args: ['-c', 'printf hi'],
          outputByteLimit: 4096,
        },
      });
      return;
    }
    if (fsEscapeMode) {
      send({
        jsonrpc: '2.0',
        id: 902,
        method: 'fs/read_text_file',
        params: { sessionId: 'sess_acp_1', path: '/etc/hosts' },
      });
      return;
    }
    if (fsRoot) {
      send({
        jsonrpc: '2.0',
        id: 902,
        method: 'fs/read_text_file',
        params: { sessionId: 'sess_acp_1', path: `${fsRoot}/input.txt` },
      });
      return;
    }
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

  if (id === 904) {
    if (message.error) {
      finishPrompt('terminal create failed');
      return;
    }
    terminalId = message.result?.terminalId ?? null;
    send({
      jsonrpc: '2.0',
      id: 905,
      method: 'terminal/wait_for_exit',
      params: { sessionId: 'sess_acp_1', terminalId },
    });
    return;
  }

  if (id === 905) {
    if (message.error) {
      finishPrompt('terminal wait failed');
      return;
    }
    send({
      jsonrpc: '2.0',
      id: 906,
      method: 'terminal/output',
      params: { sessionId: 'sess_acp_1', terminalId },
    });
    return;
  }

  if (id === 906) {
    if (message.error) {
      finishPrompt('terminal output failed');
      return;
    }
    terminalOutput = message.result?.output ?? '';
    send({
      jsonrpc: '2.0',
      id: 907,
      method: 'terminal/release',
      params: { sessionId: 'sess_acp_1', terminalId },
    });
    return;
  }

  if (id === 907) {
    finishPrompt(`terminal:${terminalOutput} advertised=${clientTerminal.enabled}`);
    return;
  }

  if (id === 902) {
    if (message.error) {
      finishPrompt('fs denied');
      return;
    }
    if (fsEscapeMode) {
      finishPrompt('fs allowed');
      return;
    }
    send({
      jsonrpc: '2.0',
      id: 903,
      method: 'fs/write_text_file',
      params: {
        sessionId: 'sess_acp_1',
        path: `${fsRoot}/output.txt`,
        content: `echo:${message.result?.content ?? ''}`,
      },
    });
    return;
  }

  if (id === 903) {
    finishPrompt(message.error ? 'fs write denied' : 'fs done');
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
