const state={key:null,cursor:null,traceFilters:{},middleware:null,fixtures:[],cliproxyapi:null};
const TURN_PAGE_SIZE=20;
const $=selector=>document.querySelector(selector);
const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,character=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[character]));
const display=value=>value===null||value===undefined||value===''?'—':String(value);
const parseList=value=>Array.isArray(value)?value:(typeof value==='string'?(()=>{try{return JSON.parse(value)}catch{return []}})():[]);
const formatTime=value=>value?new Date(value).toLocaleString():'—';

async function bootstrap(){
  try{
    const response=await fetch('/connect');
    if(!response.ok)throw new Error('Open this UI from the same machine as Alex.');
    state.key=(await response.json()).api_key;
    await Promise.all([loadStatus(),loadAccounts(),loadCLIProxyAPI()]);
  }catch(error){setDaemon(false,error.message)}
}

async function api(path,options={}){
  const headers={...(options.headers||{}),'x-api-key':state.key};
  if(options.body&&!headers['content-type'])headers['content-type']='application/json';
  const response=await fetch(path,{...options,headers});
  const payload=response.status===204?null:await response.json().catch(()=>({error:{message:response.statusText}}));
  if(!response.ok)throw new Error(payload?.error?.message||`HTTP ${response.status}`);
  return payload;
}

async function apiText(path){
  const response=await fetch(path,{headers:{'x-api-key':state.key}});
  const text=await response.text();
  if(!response.ok){
    let message=text;
    try{message=JSON.parse(text)?.error?.message||text}catch{}
    throw new Error(message||`HTTP ${response.status}`);
  }
  return text;
}

function setDaemon(ok,message){const node=$('#daemon-status');node.className=`chip ${ok?'ok':'error'}`;node.textContent=message||'Daemon online'}

async function loadStatus(){
  try{
    const [health,accounts,middleware]=await Promise.all([fetch('/health').then(response=>response.json()),api('/admin/accounts'),api('/admin/middleware')]);
    setDaemon(true,`Daemon ${health.version}`);
    const rows=[['Uptime',`${health.uptime_s}s`],['Accounts',accounts.accounts?.length||0],['Middleware rules',middleware.rules?.length||0],['In flight',health.in_flight||0]];
    $('#status-cards').innerHTML=rows.map(([name,value])=>`<div class="card"><span class="muted">${escapeHtml(name)}</span><strong>${escapeHtml(value)}</strong></div>`).join('');
  }catch(error){setDaemon(false,error.message)}
}

async function loadAccounts(){
  if(!state.key)return;
  const data=await api('/admin/accounts');
  const accounts=data.accounts||[];
  $('#accounts').innerHTML=accounts.length?accounts.map(account=>`<div class="card"><span class="muted">${escapeHtml(account.provider)}</span><strong>${escapeHtml(account.email||account.label||account.name)}</strong><small>${escapeHtml(account.health||account.status||'configured')}</small></div>`).join(''):'<div class="card"><strong>No provider connected</strong><span class="muted">Pick one below to begin.</span></div>';
  const connected=new Map();
  for(const account of accounts)connected.set(account.provider,(connected.get(account.provider)||0)+1);
  const providers=[['claude','Anthropic','oauth'],['codex','OpenAI','oauth'],['gemini','Google','oauth'],['grok','xAI','oauth'],['kimi','Moonshot','oauth'],['amp','Amp (wrap + billing)','import'],['cliproxyapi','CLIProxyAPI','form']];
  $('#providers').innerHTML=providers.map(([id,label,mode])=>`<button data-provider="${id}" data-provider-mode="${mode}"><strong>${label}</strong><span>${connected.get(id)||connected.get(id==='claude'?'anthropic':id==='codex'?'openai':id==='grok'?'xai':id)||0} connected · ${mode==='form'?'configure':mode==='import'?'import CLI key':'add another'}</span></button>`).join('');
  document.querySelectorAll('[data-provider]').forEach(button=>button.onclick=()=>button.dataset.providerMode==='form'?showProviderForm(button.dataset.provider):startLogin(button.dataset.provider));
}

function showProviderForm(provider){
  const details=$('#api-provider-details');details.open=true;
  const form=$(`#${provider}-form`);if(form){form.scrollIntoView({behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'auto':'smooth'});form.querySelector('input')?.focus()}
}

async function startLogin(provider){
  const flow=$('#login-flow');flow.hidden=false;flow.innerHTML='Starting secure login…';
  try{
    const session=await api('/admin/auth/login/start',{method:'POST',body:JSON.stringify({provider,auto_identity:true})});
    renderLogin(session);
    if(session.state==='pending')pollLogin(session.login_id);
  }catch(error){flow.innerHTML=`<strong>Could not start login</strong><p>${escapeHtml(error.message)}</p>`}
}

function renderLogin(session){
  const target=session.verification_uri_complete||session.authorize_url||session.verification_uri;
  const paste=session.mode==='paste'&&session.state==='pending'?`<form id="login-complete" class="stack"><label>Paste the authorization code or callback URL <input name="input" required autocomplete="off"></label><button type="submit">Complete login</button></form>`:'';
  $('#login-flow').innerHTML=`<strong>${escapeHtml(session.provider)} login: ${escapeHtml(session.state)}</strong>${session.user_code?`<p>Code: <code>${escapeHtml(session.user_code)}</code></p>`:''}${target?`<p><a href="${escapeHtml(target)}" target="_blank" rel="noopener">Open authorization page</a></p>`:''}${paste}${session.error?`<p>${escapeHtml(session.error)}</p>`:''}`;
  const form=$('#login-complete');if(form)form.onsubmit=event=>completeLogin(event,session.login_id);
}

async function completeLogin(event,id){event.preventDefault();const input=new FormData(event.currentTarget).get('input');try{const session=await api('/admin/auth/login/complete',{method:'POST',body:JSON.stringify({login_id:id,input})});renderLogin(session);if(session.state==='done')await Promise.all([loadAccounts(),loadStatus()])}catch(error){alert(error.message)}}
async function pollLogin(id){for(let attempt=0;attempt<180;attempt++){await new Promise(resolve=>setTimeout(resolve,2000));try{const session=await api(`/admin/auth/login/${encodeURIComponent(id)}`);renderLogin(session);if(session.state!=='pending'){await Promise.all([loadAccounts(),loadStatus()]);return}}catch(error){$('#login-flow').textContent=error.message;return}}}
async function saveOpenRouter(event){event.preventDefault();const form=new FormData(event.currentTarget);try{await api('/admin/auth/openrouter-key',{method:'POST',body:JSON.stringify(Object.fromEntries(form))});event.currentTarget.reset();await Promise.all([loadAccounts(),loadStatus()])}catch(error){alert(error.message)}}
async function saveExo(event){event.preventDefault();const url=new FormData(event.currentTarget).get('url');try{await api('/admin/exo',{method:'PUT',body:JSON.stringify({url,enabled_models:[]})});const status=await api('/admin/exo/status');if(!status.running)throw new Error(status.error||'Exo did not respond');await loadStatus()}catch(error){alert(error.message)}}

function renderCLIProxyAPI(result,message){
  state.cliproxyapi=result;
  const panel=$('#cliproxyapi-result');panel.hidden=false;
  const models=result?.models||[];
  const test=models.length?`<button id="cliproxyapi-test" type="button">Send test request</button>`:'';
  panel.innerHTML=`<strong>${escapeHtml(message||((result?.connected||result?.saved)?'CLIProxyAPI connected':'CLIProxyAPI is not connected'))}</strong>${result?.url?`<p><code>${escapeHtml(result.url)}</code></p>`:''}<p>${escapeHtml(models.length)} safe model${models.length===1?'':'s'} discovered.</p>${test}<div id="cliproxyapi-test-result" aria-live="polite"></div>`;
  const button=$('#cliproxyapi-test');if(button)button.onclick=testCLIProxyAPI;
}

async function loadCLIProxyAPI(){
  try{const result=await api('/admin/cliproxyapi');if(result.connected)renderCLIProxyAPI(result)}catch(error){console.warn('CLIProxyAPI status unavailable',error)}
}

async function saveCLIProxyAPI(event){
  event.preventDefault();const form=event.currentTarget;const values=new FormData(form);const panel=$('#cliproxyapi-result');panel.hidden=false;panel.textContent='Probing CLIProxyAPI…';
  try{
    const result=await api('/admin/auth/cliproxyapi',{method:'POST',body:JSON.stringify({url:values.get('url'),credential:values.get('credential')})});
    form.elements.credential.value='';renderCLIProxyAPI({...result,connected:true},'CLIProxyAPI connected and ready');
    await Promise.all([loadAccounts(),loadStatus()]);
  }catch(error){panel.innerHTML=`<strong>CLIProxyAPI connection failed</strong><p class="error-text">${escapeHtml(error.message)}</p>`}
}

async function testCLIProxyAPI(){
  const model=state.cliproxyapi?.models?.[0];const output=$('#cliproxyapi-test-result');if(!model||!output)return;
  output.innerHTML='<p class="muted">Sending a routed test request…</p>';
  try{
    const response=await fetch('/v1/chat/completions',{method:'POST',headers:{'authorization':`Bearer ${state.key}`,'content-type':'application/json','x-alex-harness':'shared-web-onboarding'},body:JSON.stringify({model:`cliproxyapi/${model}`,stream:false,messages:[{role:'user',content:'Reply with: Alex CLIProxyAPI test received.'}]})});
    const payload=await response.json().catch(()=>null);const trace=response.headers.get('x-alex-trace-id');
    if(!response.ok)throw new Error(payload?.error?.message||`HTTP ${response.status}`);
    output.innerHTML=`<p class="chip ok">Test request completed</p>${trace?` <button id="cliproxyapi-open-trace" type="button">Open trace ${escapeHtml(trace)}</button>`:''}`;
    const open=$('#cliproxyapi-open-trace');if(open)open.onclick=()=>showTrace(trace);
  }catch(error){output.innerHTML=`<p class="error-text">Test request failed: ${escapeHtml(error.message)}</p>`}
}

function showTrace(id){
  const tab=document.querySelector('nav button[data-view="traces"]');if(tab)tab.click();openTrace(id);
}

function writableRule(rule){
  const {api_version,built_in,hit_count,last_matched_ms,...payload}=rule;
  return payload;
}

function populatedEntries(value){
  return Object.entries(value||{}).filter(([,item])=>Array.isArray(item)?item.length:item!==null&&item!==undefined&&item!==false&&item!=='');
}

function ruleExplanation(rule){
  const conditions=populatedEntries(rule.when).map(([name,value])=>`${name.replaceAll('_',' ')}: ${Array.isArray(value)?value.join(', '):JSON.stringify(value)}`);
  if(rule.expression)conditions.push('advanced expression');
  const actions=populatedEntries(rule.then).map(([name,value])=>{
    if(name==='continue')return 'continue';
    if(name==='return_original')return 'return the original response';
    if(name==='retry_same_route')return `retry same route (${value.reason||'next eligible account'})`;
    if(name==='reroute')return `reroute${value.model?` to ${value.model}`:value.equivalent_class?` to equivalent ${value.equivalent_class}`:''}`;
    return name.replaceAll('_',' ');
  });
  return `${conditions.length?`When ${conditions.join('; ')}`:'For every matching hook'} → ${actions.join(', ')||'no action'}.`;
}

function fixtureOptions(fixtures){
  return fixtures.map(fixture=>`<option value="${escapeHtml(fixture.name)}">${escapeHtml(fixture.name)} · ${escapeHtml(fixture.provider)} ${escapeHtml(fixture.status)}</option>`).join('');
}

function renderMiddlewareRule(rule){
  const source=JSON.stringify(writableRule(rule),null,2);
  return `<article class="rule-card" data-rule-card="${escapeHtml(rule.id)}">
    <div class="section-heading rule-heading">
      <div><div class="rule-title"><h3>${escapeHtml(rule.name)}</h3><span class="chip">${rule.built_in?'Built-in':'User rule'}</span><span class="chip ${rule.enabled?'ok':''}">${rule.enabled?'Enabled':'Disabled'}</span></div><code>${escapeHtml(rule.id)}</code></div>
      <button data-rule-toggle="${escapeHtml(rule.id)}" aria-pressed="${rule.enabled}">${rule.enabled?'Disable':'Enable'}</button>
    </div>
    <p>${escapeHtml(rule.description||'No description provided.')}</p>
    <p class="explanation">${escapeHtml(ruleExplanation(rule))}</p>
    <p class="muted">Priority ${escapeHtml(rule.priority)} · ${escapeHtml(rule.hook)} · ${escapeHtml(rule.hit_count||0)} matches${rule.last_matched_ms?` · last ${escapeHtml(formatTime(rule.last_matched_ms))}`:''}</p>
    <details><summary>Readable rule source</summary><pre>${escapeHtml(source)}</pre></details>
    <form class="dry-run" data-rule-test="${escapeHtml(rule.id)}">
      <label>Saved error fixture <select name="fixture" required>${fixtureOptions(state.fixtures)}</select></label>
      <button type="submit" ${state.fixtures.length?'':'disabled'}>Run dry test</button>
    </form>
    <div class="dry-run-result" data-rule-result="${escapeHtml(rule.id)}" aria-live="polite"></div>
  </article>`;
}

function ruleResult(id){return [...document.querySelectorAll('[data-rule-result]')].find(node=>node.dataset.ruleResult===id)}

async function loadMiddleware(){
  const list=$('#middleware-rules');list.innerHTML='<div class="card">Loading middleware…</div>';
  try{
    const [middleware,fixtureData]=await Promise.all([api('/admin/middleware'),api('/admin/fixtures')]);
    state.middleware=middleware;state.fixtures=fixtureData.fixtures||[];
    const enabled=(middleware.rules||[]).filter(rule=>rule.enabled).length;
    $('#middleware-summary').innerHTML=[['Generation',middleware.generation],['Enabled',`${enabled}/${middleware.rules?.length||0}`],['Fixtures',state.fixtures.length],['Active leases',middleware.leases?.length||0]].map(([name,value])=>`<div class="card"><span class="muted">${escapeHtml(name)}</span><strong>${escapeHtml(value)}</strong></div>`).join('');
    const errors=middleware.errors||[];
    const errorPanel=$('#middleware-errors');errorPanel.hidden=!errors.length;errorPanel.innerHTML=errors.length?`<strong>Runtime errors</strong><ul>${errors.map(error=>`<li>${escapeHtml(error)}</li>`).join('')}</ul>`:'';
    list.innerHTML=(middleware.rules||[]).map(renderMiddlewareRule).join('')||'<div class="card">No middleware rules installed.</div>';
    document.querySelectorAll('[data-rule-toggle]').forEach(button=>button.onclick=()=>setRuleEnabled(button.dataset.ruleToggle,!JSON.parse(button.getAttribute('aria-pressed'))));
    document.querySelectorAll('[data-rule-test]').forEach(form=>form.onsubmit=event=>dryRunRule(event,form.dataset.ruleTest));
  }catch(error){list.innerHTML=`<div class="flow"><strong>Could not load middleware</strong><p>${escapeHtml(error.message)}</p></div>`}
}

async function setRuleEnabled(id,enabled){
  const rule=state.middleware?.rules?.find(candidate=>candidate.id===id);
  if(!rule)return;
  const button=[...document.querySelectorAll('[data-rule-toggle]')].find(node=>node.dataset.ruleToggle===id);
  if(button)button.disabled=true;
  try{
    await api(`/admin/middleware/rules/${encodeURIComponent(id)}`,{method:'PUT',body:JSON.stringify({...writableRule(rule),enabled})});
    await Promise.all([loadMiddleware(),loadStatus()]);
  }catch(error){if(button)button.disabled=false;alert(error.message)}
}

async function dryRunRule(event,id){
  event.preventDefault();
  const fixture=new FormData(event.currentTarget).get('fixture');
  const output=ruleResult(id);output.innerHTML='<p class="muted">Evaluating fixture…</p>';
  try{
    const result=await api('/admin/middleware/test',{method:'POST',body:JSON.stringify({middleware_id:id,fixture_name:fixture})});
    const decision=result.decision?.decision||'continue';
    const matched=(result.records||[]).filter(record=>record.state==='matched').map(record=>record.rule_id).join(', ')||'none';
    output.innerHTML=`<div class="dry-run-summary"><strong>Decision: ${escapeHtml(decision.replaceAll('_',' '))}</strong><span>Matched: ${escapeHtml(matched)}</span><span>Body inspection: ${result.body_inspection_required?'required':'not required'}</span></div><details><summary>Full dry-run result</summary><pre>${escapeHtml(JSON.stringify(result,null,2))}</pre></details>`;
  }catch(error){output.innerHTML=`<p class="error-text">${escapeHtml(error.message)}</p>`}
}

function traceQuery(append){
  const params=new URLSearchParams({limit:'25',...state.traceFilters});
  if(append&&state.cursor){params.set('before_ms',state.cursor.before_ms);params.set('before_id',state.cursor.before_id)}
  return params;
}

async function loadTraces(append=false){
  const data=await api(`/traces/summaries?${traceQuery(append)}`);state.cursor=data.next_cursor;
  const list=$('#trace-list');if(!append)list.innerHTML='';
  for(const trace of data.traces||[]){
    const button=document.createElement('button');button.className=`trace-row ${trace.status>=400||trace.error?'error':''}`;
    button.innerHTML=`<code>${escapeHtml(trace.model||trace.id)}</code><span>${escapeHtml(trace.provider||'unrouted')} · ${escapeHtml(trace.harness||'unknown harness')}<small>${escapeHtml(formatTime(trace.ts_request_ms))}</small></span><span>${escapeHtml(trace.status??'—')}</span>`;
    button.onclick=()=>openTrace(trace.id);list.append(button);
  }
  if(!list.children.length)list.innerHTML='<div class="card">No matching traces. Route one request through Alex or change the metadata filters.</div>';
  $('#more-traces').hidden=!data.has_more;
}

function facts(items){return `<dl class="facts">${items.map(([label,value])=>`<div><dt>${escapeHtml(label)}</dt><dd>${escapeHtml(display(value))}</dd></div>`).join('')}</dl>`}

function middlewareRecords(attempt){
  const records=parseList(attempt.middleware_decisions);
  if(!records.length)return '<p class="muted">No middleware decisions recorded.</p>';
  return `<ul class="decision-list">${records.map(record=>`<li><code>${escapeHtml(record.rule_name||record.rule_id||'unknown rule')}</code><span class="chip ${record.state==='matched'?'ok':''}">${escapeHtml(record.state||'unknown')}</span>${record.action?`<span>${escapeHtml(record.action)}</span>`:''}${record.suppressed?'<span class="error-text">suppressed</span>':''}${record.explanation?`<span>${escapeHtml(record.explanation)}</span>`:''}</li>`).join('')}</ul>`;
}

function renderAttempt(attempt,index){
  return `<article class="attempt"><h4>Attempt ${escapeHtml(attempt.attempt_number||attempt.attempt||index+1)}</h4>${facts([
    ['Provider',attempt.provider||attempt.upstream_provider],['Model',attempt.model||attempt.routed_model],['Account',attempt.account_id],['Status',attempt.status],['Error',attempt.error||attempt.error_kind],['Latency',attempt.latency_ms!==undefined?`${attempt.latency_ms} ms`:null]
  ])}<h5>Middleware decisions</h5>${middlewareRecords(attempt)}<details><summary>Attempt source</summary><pre>${escapeHtml(JSON.stringify(attempt,null,2))}</pre></details></article>`;
}

function bodyDetails(trace){
  const bodies=[['request','Client request',trace.req_body_path],['upstream-request','Upstream request',trace.upstream_req_body_path],['response','Client response',trace.resp_body_path]];
  if(trace.via_dario){bodies.push(['dario-upstream-request','Dario upstream request',true],['dario-upstream-response','Dario upstream response',true])}
  return bodies.filter(([, ,available])=>available).map(([kind,label])=>`<details class="lazy-data" data-body-kind="${kind}"><summary>${escapeHtml(label)}</summary><pre>Open to load this body.</pre></details>`).join('')||'<p class="muted">No stored bodies are available for this trace.</p>';
}

function renderTraceDetail(id,data){
  const trace=data.trace||data;
  const attempts=parseList(trace.attempts);
  const detail=$('#trace-detail');detail.hidden=false;
  detail.innerHTML=`<div class="section-heading"><div><h3>Trace ${escapeHtml(id)}</h3><span class="muted">${escapeHtml(formatTime(trace.ts_request_ms))}</span></div><button id="close-detail">Close</button></div>
    <h4>Summary</h4>${facts([['Status',trace.status],['Latency',trace.latency_ms!==null&&trace.latency_ms!==undefined?`${trace.latency_ms} ms`:null],['Input tokens',trace.input_tokens],['Output tokens',trace.output_tokens],['Error',trace.error||trace.error_kind],['Session',trace.session_id],['Run',trace.run_id]])}
    <h4>Provenance</h4>${facts([['Harness',trace.harness],['Client format',trace.client_format],['Provider',trace.upstream_provider],['Upstream format',trace.upstream_format],['Requested model',trace.requested_model],['Routed model',trace.routed_model],['Original model',trace.original_model],['Served model',trace.served_model],['Account',trace.account_id],['Original account',trace.original_account_id],['Served account',trace.served_account_id],['Via Dario',trace.via_dario],['Dario generation',trace.dario_generation],['Routing explanation',trace.substitution_reason]])}
    <h4>Attempts and middleware</h4><div class="attempt-list">${attempts.length?attempts.map(renderAttempt).join(''):'<p class="muted">No attempt records stored.</p>'}</div>
    <h4>Stored bodies</h4><div class="lazy-list">${bodyDetails(trace)}</div>
    ${trace.session_id?`<h4>Session</h4><details class="lazy-data" data-transcript="${escapeHtml(trace.session_id)}"><summary>Conversation turns</summary><div>Open to load session turns.</div></details>`:''}`;
  $('#close-detail').onclick=()=>detail.hidden=true;
  detail.querySelectorAll('[data-body-kind]').forEach(node=>node.ontoggle=()=>{if(node.open&&!node.dataset.loaded)loadTraceBody(id,node)});
  detail.querySelectorAll('[data-transcript]').forEach(node=>node.ontoggle=()=>{if(node.open&&!node.dataset.loaded)loadTranscript(node)});
  detail.scrollIntoView({behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'auto':'smooth'});
}

// Metadata is a body-free endpoint. Body bytes are requested only by the
// explicit details-toggle handlers below, never by trace list or detail load.
async function openTrace(id){
  try{renderTraceDetail(id,await api(`/traces/${encodeURIComponent(id)}/metadata`))}
  catch(error){alert(error.message)}
}

async function loadTraceBody(id,node){
  node.dataset.loaded='true';const output=node.querySelector('pre');output.textContent='Loading…';
  try{output.textContent=await apiText(`/traces/${encodeURIComponent(id)}/body/${encodeURIComponent(node.dataset.bodyKind)}`)}catch(error){output.textContent=`Could not load body: ${error.message}`}
}

function renderExecutedTools(tools){
  if(!tools?.length)return '';
  return `<div class="executed-tools"><h6>Executed tools</h6>${tools.map(tool=>{const args=typeof tool.arguments==='string'?tool.arguments:JSON.stringify(tool.arguments,null,2);return `<details><summary>${escapeHtml(tool.tool_name||'tool')} ${tool.is_error?'<span class="error-text">error</span>':''}</summary>${args?`<strong>Arguments</strong><pre>${escapeHtml(args)}</pre>`:''}${tool.result?`<strong>Result</strong><pre>${escapeHtml(tool.result)}</pre>`:''}</details>`}).join('')}</div>`;
}

function renderTurn(turn){
  const assistant=turn.assistant||parseList(turn.assistant_blocks).filter(block=>block.type==='text').map(block=>block.text).join('\n\n');
  return `<article class="turn">${turn.user?`<div><strong>User</strong><pre>${escapeHtml(turn.user)}</pre></div>`:''}${assistant?`<div><strong>Assistant</strong><pre>${escapeHtml(assistant)}</pre></div>`:''}${renderExecutedTools(turn.executed_tools)}${facts([['Trace',turn.trace_id],['Model',turn.model||turn.served_model],['Status',turn.status],['Input tokens',turn.input_tokens],['Output tokens',turn.output_tokens]])}</article>`;
}

function renderTurnSummary(turn){
  return `<details class="turn-summary" data-turn-trace="${escapeHtml(turn.trace_id)}"><summary><span><code>${escapeHtml(turn.model||turn.trace_id)}</code> · ${escapeHtml(turn.provider||'unrouted')}</span><span>${escapeHtml(turn.status??'—')} · ${escapeHtml(formatTime(turn.ts_request_ms))}</span></summary><div class="turn-detail muted">Open to load only this turn.</div></details>`;
}

function replaceTranscriptPage(target,html){target.replaceChildren();target.insertAdjacentHTML('afterbegin',html)}

async function loadTranscriptTurn(node){
  node.dataset.loaded='true';const target=node.querySelector('.turn-detail');target.textContent='Loading this turn…';
  try{const data=await api(`/traces/${encodeURIComponent(node.dataset.turnTrace)}/turn`);target.classList.remove('muted');target.innerHTML=renderTurn(data.turn)}catch(error){delete node.dataset.loaded;target.textContent=`Could not load turn: ${error.message}`}
}

async function loadTranscriptPage(node,cursor){
  const target=node.querySelector('.session-turns');target.textContent='Loading a bounded page…';
  const params=new URLSearchParams({limit:String(TURN_PAGE_SIZE)});if(cursor){params.set('after_ms',cursor.after_ms);params.set('after_id',cursor.after_id)}
  try{
    const data=await api(`/traces/sessions/${encodeURIComponent(node.dataset.transcript)}/transcript/page?${params}`);
    const turns=(data.turns||[]).map(renderTurnSummary).join('')||'<p class="muted">No turns found.</p>';
    const controls=`<div class="turn-page-controls"><button data-turn-previous ${node._pageIndex?'':'disabled'}>Previous page</button><span>Page ${node._pageIndex+1} · up to ${TURN_PAGE_SIZE} turns</span><button data-turn-next ${data.has_more?'':'disabled'}>Next page</button></div>`;
    replaceTranscriptPage(target,`${turns}${controls}`);
    target.querySelectorAll('[data-turn-trace]').forEach(turn=>turn.ontoggle=()=>{if(turn.open&&!turn.dataset.loaded)loadTranscriptTurn(turn)});
    target.querySelector('[data-turn-previous]').onclick=()=>{if(node._pageIndex>0){node._pageIndex-=1;loadTranscriptPage(node,node._pageStarts[node._pageIndex])}};
    target.querySelector('[data-turn-next]').onclick=()=>{if(data.next_cursor){node._pageStarts=node._pageStarts.slice(0,node._pageIndex+1);node._pageStarts.push(data.next_cursor);node._pageIndex+=1;loadTranscriptPage(node,data.next_cursor)}};
  }catch(error){target.textContent=`Could not load turns: ${error.message}`}
}

async function loadTranscript(node){node.dataset.loaded='true';node._pageStarts=[null];node._pageIndex=0;const holder=node.querySelector('div');holder.className='session-turns';await loadTranscriptPage(node,null)}

function applyTraceFilters(event){
  event.preventDefault();const values=new FormData(event.currentTarget);state.traceFilters={};
  for(const [key,value] of values.entries())if(value&&String(value).trim())state.traceFilters[key]=key==='errors'?'1':String(value).trim();
  state.cursor=null;loadTraces(false);
}

document.querySelectorAll('nav button').forEach(button=>button.onclick=()=>{
  document.querySelectorAll('nav button').forEach(item=>item.removeAttribute('aria-current'));button.setAttribute('aria-current','page');
  document.querySelectorAll('[data-panel]').forEach(panel=>panel.hidden=panel.id!==`${button.dataset.view}-view`);
  if(button.dataset.view==='traces')loadTraces(false);
  if(button.dataset.view==='middleware')loadMiddleware();
});
$('#refresh-middleware').onclick=loadMiddleware;
$('#refresh-traces').onclick=()=>loadTraces(false);
$('#more-traces').onclick=()=>loadTraces(true);
$('#trace-filters').onsubmit=applyTraceFilters;
$('#trace-filters').onreset=()=>{state.traceFilters={};state.cursor=null;setTimeout(()=>loadTraces(false),0)};
$('#openrouter-form').onsubmit=saveOpenRouter;
$('#exo-form').onsubmit=saveExo;
$('#cliproxyapi-form').onsubmit=saveCLIProxyAPI;
bootstrap();
