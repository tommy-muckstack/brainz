import assert from 'node:assert/strict';
import { test } from 'node:test';
import { installMcpSelection, MCP_SELECTION_CONFIG, MCP_SERVERS_CONFIG } from './claude_mcp_selection.mjs';

function fixture() {
  const applied = [];
  const calls = [];
  const servers = [{name:'notes',status:'connected'},{name:'calendar',status:'connected'}];
  class Agent {
    async initialize() { return {agentCapabilities:{_meta:{existing:true}}}; }
    async setSessionConfigOption(params) { calls.push(params); return {configOptions:[]}; }
    sessions = { one:{
      configOptions:[{id:'model'}],
      creationParams:{_meta:{claudeCode:{options:{settings:{permissions:{deny:['Bash(rm:*)'],allow:['Read']}}}}}},
      query:{ async mcpServerStatus(){ return servers; },async applyFlagSettings(settings){applied.push(settings);} },
    }};
  }
  installMcpSelection(Agent);
  return {agent:new Agent(), applied, calls, servers};
}

test('selection overlays session tools and All restores the original rules', async () => {
  const {agent,applied,servers} = fixture();
  const capability = await agent.initialize({});
  assert.deepEqual(capability.agentCapabilities._meta,{existing:true,brainzMcpSelection:1});
  const set = selected => agent.setSessionConfigOption({sessionId:'one',configId:MCP_SELECTION_CONFIG,value:JSON.stringify(selected)});
  await set(['notes']);
  assert.deepEqual(applied[0],{permissions:{deny:['Bash(rm:*)','mcp__calendar__*'],allow:['Read']}});
  await set([]);
  assert.deepEqual(applied[1].permissions.deny,['Bash(rm:*)','mcp__*']);
  await set(null);
  assert.deepEqual(applied[2],{permissions:{deny:['Bash(rm:*)'],allow:['Read']}});
  assert(servers.every(server=>server.status==='connected'));
});

test('lists all live server sources without applying settings',async()=>{
  const {agent,applied}=fixture();
  const result=await agent.setSessionConfigOption({sessionId:'one',configId:MCP_SERVERS_CONFIG,value:'list'});
  assert.deepEqual(result._meta.brainzMcpServers,['calendar','notes']);
  assert.equal(applied.length,0);
});

test('normal configuration passes through and unknown selections fail before applying',async()=>{
  const {agent,applied,calls}=fixture();
  const request={sessionId:'one',configId:'model',value:'chosen'};
  await agent.setSessionConfigOption(request);
  assert.deepEqual(calls,[request]);
  await assert.rejects(agent.setSessionConfigOption({sessionId:'one',configId:MCP_SELECTION_CONFIG,value:'["missing"]'}),/no longer configured/);
  await assert.rejects(agent.setSessionConfigOption({sessionId:'one',configId:MCP_SELECTION_CONFIG,value:'{}'}),/Invalid MCP selection/);
  assert.equal(applied.length,0);
});

test('each query owns its own filter and failure is returned to the client',async()=>{
  const {agent,applied}=fixture();
  const other=[];
  agent.sessions.two={configOptions:[],query:{async mcpServerStatus(){return [{name:'notes',status:'connected'}];},async applyFlagSettings(value){other.push(value);}}};
  await agent.setSessionConfigOption({sessionId:'two',configId:MCP_SELECTION_CONFIG,value:'[]'});
  assert.equal(applied.length,0);
  assert.deepEqual(other,[{permissions:{deny:['mcp__*']}}]);
  agent.sessions.two.query.applyFlagSettings=async()=>{throw new Error('filter failed');};
  await assert.rejects(agent.setSessionConfigOption({sessionId:'two',configId:MCP_SELECTION_CONFIG,value:'null'}),/filter failed/);
});

test('concurrent requests apply in order even if an earlier client stops waiting', async () => {
  const { agent, applied } = fixture();
  let release;
  let started;
  const firstStarted = new Promise(resolve => { started = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  agent.sessions.one.query.applyFlagSettings = async settings => {
    if (applied.length === 0) {
      started();
      await gate;
    }
    applied.push(settings);
  };
  const set = value => agent.setSessionConfigOption({sessionId:'one',configId:MCP_SELECTION_CONFIG,value});
  const first = set('[]');
  await firstStarted;
  const second = set('null');
  release();
  await Promise.all([first, second]);
  assert.deepEqual(applied.map(settings => settings.permissions.deny), [
    ['Bash(rm:*)', 'mcp__*'], ['Bash(rm:*)'],
  ]);
});
