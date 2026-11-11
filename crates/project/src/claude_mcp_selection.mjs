import { access, readFile, realpath } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const MCP_SELECTION_CAPABILITY = 'brainzMcpSelection';
export const MCP_SELECTION_CONFIG = 'brainz/mcp-selection';
export const MCP_SERVERS_CONFIG = 'brainz/mcp-servers';

export function installMcpSelection(Agent) {
  const initialize = Agent.prototype.initialize;
  const setConfig = Agent.prototype.setSessionConfigOption;
  if (typeof initialize !== 'function' || typeof setConfig !== 'function') {
    throw new Error('The installed Claude adapter does not expose session configuration');
  }
  const baselines = new WeakMap();
  const pending = new WeakMap();
  Agent.prototype.initialize = async function (request) {
    const result = await initialize.call(this, request);
    result.agentCapabilities._meta = {
      ...result.agentCapabilities._meta,
      [MCP_SELECTION_CAPABILITY]: 1,
    };
    return result;
  };
  Agent.prototype.setSessionConfigOption = async function (params) {
    if (params.configId !== MCP_SELECTION_CONFIG && params.configId !== MCP_SERVERS_CONFIG) {
      return setConfig.call(this, params);
    }
    const session = this.sessions[params.sessionId];
    if (!session || session.queryClosed) throw new Error('The Claude session has ended');
    const query = session.query;
    if (typeof query.applyFlagSettings !== 'function' || typeof query.mcpServerStatus !== 'function') {
      throw new Error('Update the Claude agent to use the MCP picker');
    }
    const apply = async () => {
      const servers = await query.mcpServerStatus();
      const names = [...new Set(servers.map(server => server.name))].sort();
      if (params.configId === MCP_SELECTION_CONFIG) {
        const selected = JSON.parse(params.value);
        if (selected !== null && (!Array.isArray(selected) || selected.some(name => typeof name !== 'string'))) {
          throw new Error('Invalid MCP selection');
        }
        if (selected?.some(name => !names.includes(name))) {
          throw new Error('A selected MCP is no longer configured. Reopen the MCP picker.');
        }
        if (!baselines.has(query)) {
          let settings = session.creationParams?._meta?.claudeCode?.options?.settings;
          if (typeof settings === 'string') {
            settings = JSON.parse(settings.trimStart().startsWith('{') ? settings : await readFile(resolve(session.creationParams.cwd ?? process.cwd(), settings), 'utf8'));
          }
          baselines.set(query, structuredClone(settings?.permissions ?? null));
        }
        const baseline = baselines.get(query);
        const blocked = selected === null ? [] : names.filter(name => !selected.includes(name));
        if (selected?.length && blocked.some(name => !/^[a-zA-Z0-9_-]+$/.test(name))) {
          throw new Error('An MCP name cannot be represented safely in a tool filter');
        }
        const deny = selected?.length === 0 ? ['mcp__*'] : blocked.map(name => `mcp__${name}__*`);
        await query.applyFlagSettings({
          permissions: deny.length ? { ...baseline, deny: [...(baseline?.deny ?? []), ...deny] } : baseline,
        });
      }
      return { configOptions: session.configOptions, _meta: { brainzMcpServers: names } };
    };
    // A cancelled client request can still be executing in the adapter. Keep
    // subsequent filters ordered so an older request cannot win the race.
    const operation = (pending.get(query) ?? Promise.resolve()).then(apply, apply);
    pending.set(query, operation);
    try {
      return await operation;
    } finally {
      if (pending.get(query) === operation) pending.delete(query);
    }
  };
}

// Preload the exported adapter class before its normal CLI entry point starts.
// This keeps upstream authentication, transport, and lifecycle handling intact.
if (process.env.BRAINZ_CLAUDE_MCP_PICKER === '1') {
  try {
    const entry = pathToFileURL(await realpath(process.argv[1]));
    let module = new URL('./acp-agent.js', entry);
    try {
      await access(module);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      module = new URL('./v1/acp-agent.js', entry);
    }
    const { ClaudeAcpAgent } = await import(module);
    installMcpSelection(ClaudeAcpAgent);
  } catch (error) {
    console.error(`Brainz MCP picker unavailable: ${error.message}`);
  }
}
