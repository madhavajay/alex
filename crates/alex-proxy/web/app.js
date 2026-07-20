const state={key:null,cursor:null};
const $=s=>document.querySelector(s);
const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

async function bootstrap(){
  try{
    const response=await fetch('/connect');
    if(!response.ok)throw new Error('Open this UI from the same machine as Alex.');
    state.key=(await response.json()).api_key;
    await Promise.all([loadStatus(),loadAccounts()]);
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
function setDaemon(ok,message){const node=$('#daemon-status');node.className=`chip ${ok?'ok':'error'}`;node.textContent=message||'Daemon online'}
async function loadStatus(){
  try{
    const [health,accounts,middleware]=await Promise.all([fetch('/health').then(r=>r.json()),api('/admin/accounts'),api('/admin/middleware')]);
    setDaemon(true,`Daemon ${health.version}`);
    const rows=[['Uptime',`${health.uptime_s}s`],['Accounts',accounts.accounts?.length||0],['Middleware rules',middleware.rules?.length||0],['In flight',health.in_flight||0]];
    $('#status-cards').innerHTML=rows.map(([name,value])=>`<div class="card"><span class="muted">${escapeHtml(name)}</span><strong>${escapeHtml(value)}</strong></div>`).join('');
  }catch(error){setDaemon(false,error.message)}
}
async function loadAccounts(){
  if(!state.key)return;
  const data=await api('/admin/accounts');
  const accounts=data.accounts||[];
  $('#accounts').innerHTML=accounts.length?accounts.map(a=>`<div class="card"><span class="muted">${escapeHtml(a.provider)}</span><strong>${escapeHtml(a.email||a.label||a.name)}</strong><small>${escapeHtml(a.health||a.status||'configured')}</small></div>`).join(''):'<div class="card"><strong>No provider connected</strong><span class="muted">Pick one below to begin.</span></div>';
  const connected=new Map();
  for(const account of accounts)connected.set(account.provider,(connected.get(account.provider)||0)+1);
  const providers=[['claude','Anthropic'],['codex','OpenAI'],['gemini','Google'],['grok','xAI'],['kimi','Moonshot'],['amp','Amp']];
  $('#providers').innerHTML=providers.map(([id,label])=>`<button data-provider="${id}"><strong>${label}</strong><span>${connected.get(id)||connected.get(id==='claude'?'anthropic':id==='codex'?'openai':id==='grok'?'xai':id)||0} connected · add another</span></button>`).join('');
  document.querySelectorAll('[data-provider]').forEach(button=>button.onclick=()=>startLogin(button.dataset.provider));
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
async function pollLogin(id){
  for(let attempt=0;attempt<180;attempt++){
    await new Promise(resolve=>setTimeout(resolve,2000));
    try{const session=await api(`/admin/auth/login/${encodeURIComponent(id)}`);renderLogin(session);if(session.state!=='pending'){await Promise.all([loadAccounts(),loadStatus()]);return}}catch(error){$('#login-flow').textContent=error.message;return}
  }
}
async function saveOpenRouter(event){event.preventDefault();const form=new FormData(event.currentTarget);try{await api('/admin/auth/openrouter-key',{method:'POST',body:JSON.stringify(Object.fromEntries(form))});event.currentTarget.reset();await Promise.all([loadAccounts(),loadStatus()])}catch(error){alert(error.message)}}
async function saveExo(event){event.preventDefault();const url=new FormData(event.currentTarget).get('url');try{await api('/admin/exo',{method:'PUT',body:JSON.stringify({url,enabled_models:[]})});const status=await api('/admin/exo/status');if(!status.running)throw new Error(status.error||'Exo did not respond');await loadStatus()}catch(error){alert(error.message)}}

async function loadTraces(append=false){
  const params=new URLSearchParams({limit:'25'});if(append&&state.cursor){params.set('before_ms',state.cursor.before_ms);params.set('before_id',state.cursor.before_id)}
  const data=await api(`/traces/summaries?${params}`);state.cursor=data.next_cursor;
  const list=$('#trace-list');if(!append)list.innerHTML='';
  for(const trace of data.traces||[]){const button=document.createElement('button');button.className=`trace-row ${trace.status>=400||trace.error?'error':''}`;button.innerHTML=`<code>${escapeHtml(trace.model||trace.id)}</code><span>${escapeHtml(trace.provider||'unrouted')} · ${escapeHtml(trace.harness||'unknown harness')}</span><span>${escapeHtml(trace.status??'—')}</span>`;button.onclick=()=>openTrace(trace.id);list.append(button)}
  if(!list.children.length)list.innerHTML='<div class="card">No traces yet. Route one request through Alex, then refresh.</div>';
  $('#more-traces').hidden=!data.has_more;
}
async function openTrace(id){const data=await api(`/traces/${encodeURIComponent(id)}`);const detail=$('#trace-detail');detail.hidden=false;detail.innerHTML=`<div class="section-heading"><h3>Trace ${escapeHtml(id)}</h3><button id="close-detail">Close</button></div><pre>${escapeHtml(JSON.stringify(data,null,2))}</pre>`;$('#close-detail').onclick=()=>detail.hidden=true;detail.scrollIntoView({behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'auto':'smooth'})}

document.querySelectorAll('nav button').forEach(button=>button.onclick=()=>{document.querySelectorAll('nav button').forEach(item=>item.removeAttribute('aria-current'));button.setAttribute('aria-current','page');document.querySelectorAll('[data-panel]').forEach(panel=>panel.hidden=panel.id!==`${button.dataset.view}-view`);if(button.dataset.view==='traces')loadTraces(false)});
$('#refresh-traces').onclick=()=>loadTraces(false);$('#more-traces').onclick=()=>loadTraces(true);$('#openrouter-form').onsubmit=saveOpenRouter;$('#exo-form').onsubmit=saveExo;
bootstrap();
