import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");
function block(start, end) { return source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start))); }
function harness({ actualWrapper = false } = {}) {
  let now = 10, active = true, focused = 7, hook = null;
  const calls = [], gates = [];
  const frame = { id:"frame", loaderId:"loader", url:"https://fixture.test/nav" };
  const snapshot = {id:"b".repeat(32),generation:"a".repeat(32),expires:30010,backendNodes:[{backendDOMNodeId:7,frameId:"frame"}]};
  const page = {pageId:"page:owned",closed:false,browserPage:{debugger_url:"ws://private/page",width:1920,height:1080,
    _inspection:{generation:snapshot.generation,snapshot,binding:"frame:loader:https://fixture.test/nav"}}};
  const cdp = {closed:false,async request(method,params={}){
    calls.push({method,params});if(hook) await hook(method,params);
    if(method==="Page.getFrameTree") return {frameTree:{frame}};
    if(method==="DOM.describeNode") return {node:{backendNodeId:params.backendNodeId??focused,nodeName:"INPUT",attributes:["type","text"]}};
    if(method==="DOM.getContentQuads") return {quads:[[10,10,100,10,100,40,10,40]]};
    if(method==="DOM.getNodeForLocation") return {backendNodeId:7};
    if(method==="DOM.getDocument") return {root:{nodeId:1}};
    if(method==="DOM.querySelector") return {nodeId:focused};
    return {};
  }};
  cdp.close=()=>{cdp.closed=true;};
  page.browserPage._cdp=cdp;
  const context=vm.createContext({Buffer,console,setTimeout,clearTimeout,performance:{now:()=>now},
    finiteCoordinate:n=>Number(n),validatePasteText:s=>s,summarizeBrowserFileChooser:()=>null,
    ensureBrowserFileChooserInterception:()=>{throw new Error("existing CDP interception must stay attached to its original client");},
    withBrowserCdp:async(browser,timeout,callback)=>{assert.equal(browser,page.browserPage);return callback(cdp);}});
  vm.runInContext(block('function withTimeout(', '\nfunction createBrowserFileChooserState')+
    block('const OPERATOR_ID =','async function withBrowserCdp(')+
    block('async function pasteTextIntoBrowserPage(', '\nasync function browserPageStateFromCdp')+
    block('async function dispatchBrowserInputEvent(', '\nfunction validateBrowserNavigationUrl'),context);
  if (actualWrapper) {
    context.CdpClient = class {
      constructor() { this.closed=false; this.request=cdp.request; }
      async connect() {}
      close() { this.closed=true; }
    };
    vm.runInContext(block('async function withBrowserCdp(', '\nfunction closeCachedBrowserCdp'),context);
  }
  context.gate=async(command,id,duration)=>{gates.push({command,id,duration});return {active:command!=="release"};};
  vm.runInContext('browserInputWriterGate = gate',context);
  const current=()=>active;
  const lease={type:"operator_lease",command:"acquire",admission_id:"c".repeat(32),document_generation:snapshot.generation,actions:["click","type"],duration_ms:30000};
  const event=(action="click",id="d")=>({type:"operator_ref",schema:"elastos.browser.ref-input/v1",request_id:id.repeat(32),admission_id:lease.admission_id,
    document_generation:snapshot.generation,ref:`${snapshot.id}:0`,action,...(action==="type"?{text:"hello"}:{})});
  return {context,page,calls,gates,frame,snapshot,lease,event,
    acquire:()=>context.browserOperatorLease(page,lease,current),
    input:ev=>context.browserRefInput(page,ev,current),
    queue:action=>context.withBrowserInputWriter(page,current,action),
    hook:fn=>{hook=fn;},advance:n=>{now+=n;},unload:()=>{active=false;},focus:n=>{focused=n;},
    close:()=>{if(page.operatorLease){clearTimeout(page.operatorLease.timer);page.operatorLease.active=false;}}};
}
const effects=h=>h.calls.filter(c=>c.method.startsWith("Input.dispatch")||c.method==="Input.insertText");

test("native ref click and text reuse existing typed Engine input; replay has no effect",async()=>{
  const h=harness();try{
    await h.acquire();const click=h.event();
    assert.equal((await h.input(click)).accepted,true);
    assert.deepEqual(effects(h).map(c=>c.params.type),["mouseMoved","mousePressed","mouseReleased"]);
    assert.equal(effects(h)[1].params.pointerType,"mouse");
    await h.input(click);assert.equal(effects(h).length,3);
    await assert.rejects(h.input({...click,ref:`${"f".repeat(32)}:0`}),/Browser operator/);
    await h.input(h.event("type","e"));assert.equal(effects(h).at(-1).method,"Input.insertText");
    assert.equal(effects(h).at(-1).params.text,"hello");
    assert.ok(h.gates.some(g=>g.command==="check"));
  }finally{h.close();}
});

for(const negative of ["foreign lease","foreign snapshot","foreign document","expired snapshot","expired lease","revoked","detached page"]){
  test(`${negative} rejects before CDP input`,async()=>{
    const h=harness();try{
      await h.acquire();let input=h.event();
      if(negative==="foreign lease")input.admission_id="f".repeat(32);
      if(negative==="foreign snapshot")input.ref=`${"f".repeat(32)}:0`;
      if(negative==="foreign document")input.document_generation="f".repeat(32);
      if(negative==="expired snapshot")h.snapshot.expires=0;
      if(negative==="expired lease")h.advance(30000);
      if(negative==="revoked")h.page.operatorLease.active=false;
      if(negative==="detached page")h.unload();
      await assert.rejects(h.input(input));assert.equal(effects(h).length,0);
    }finally{h.close();}
  });
}

test("fresh native document binding and focused node must match before text",async()=>{
  for(const changed of ["navigation","focus"]){const h=harness();try{
    await h.acquire();if(changed==="navigation")h.frame.loaderId="replaced";else h.focus(99);
    await assert.rejects(h.input(h.event("type")));assert.equal(effects(h).length,0);
  }finally{h.close();}}
});

for(const boundary of ["DOM.describeNode","DOM.getNodeForLocation","Input.dispatchMouseEvent"]){
  test(`revocation after ${boundary} prevents later effects and reports uncertainty`,async()=>{
    const h=harness();try{
      await h.acquire();h.hook(async method=>{if(method===boundary)h.page.operatorLease.active=false;});
      await assert.rejects(h.input(h.event()));
      assert.equal(effects(h).length,boundary==="Input.dispatchMouseEvent"?1:0);
    }finally{h.close();}
  });
}

test("paused event loop cannot send input after absolute action deadline",async()=>{
  const h=harness();try{
    await h.acquire();h.hook(async method=>{if(method==="DOM.getNodeForLocation")h.advance(1501);});
    await assert.rejects(h.input(h.event()));assert.equal(effects(h).length,0);
  }finally{h.close();}
});

test("HTTP writers share FIFO; an uncertain predecessor abandons queued effects",async()=>{
  const h=harness(),seen=[];let release;
  const first=h.queue(async()=>{seen.push("first");await new Promise(resolve=>{release=resolve;});throw new Error("uncertain");});
  await Promise.resolve();const next=h.queue(async()=>{seen.push("second");});
  release();await assert.rejects(first);await assert.rejects(next);assert.deepEqual(seen,["first"]);
  await h.queue(async()=>seen.push("fresh"));assert.deepEqual(seen,["first","fresh"]);
});

test("acquire failure and close race keep writer inactive",async()=>{
  const h=harness();try{
    h.context.gate=async()=>{h.unload();return {};};vm.runInContext('browserInputWriterGate = gate',h.context);
    await assert.rejects(h.acquire());assert.equal(h.page.operatorLease.active,false);assert.equal(effects(h).length,0);
  }finally{h.close();}
});

test("a queued release still runs after an uncertain writer poisons dependent input",async()=>{
  const h=harness(),seen=[];let release;
  const first=h.queue(async()=>{await new Promise(resolve=>{release=resolve;});throw new Error("uncertain");});
  await Promise.resolve();
  const cleanup=h.context.withBrowserInputWriter(h.page,()=>true,async()=>seen.push("released"),true);
  release();await assert.rejects(first);await cleanup;assert.deepEqual(seen,["released"]);
});

test("release before queued acquisition cancels acquisition without touching the native gate",async()=>{
  const h=harness();h.context.cancelBrowserOperatorLease(h.page,h.lease.admission_id);
  await assert.rejects(h.acquire());assert.equal(h.gates.length,0);
});

test("revocation during an acknowledged mouse press releases that press before handoff",async()=>{
  const h=harness();try{
    await h.acquire();h.hook(async(method,params)=>{if(method==="Input.dispatchMouseEvent"&&params.type==="mousePressed")h.page.operatorLease.active=false;});
    await assert.rejects(h.input(h.event()));
    assert.deepEqual(effects(h).map(c=>c.params.type),["mouseMoved","mousePressed","mouseReleased"]);
  }finally{h.close();}
});

for (const boundary of ["deadline", "disconnect"]) {
  test(`HTTP operator dispatch rejects queued ${boundary}; cleanup release still runs`, async () => {
    const start = source.indexOf("        const inputStarted = performance.now();");
    const end = source.indexOf("        if (ownerHandoff) await", start);
    assert.ok(start > 0 && end > start);
    const body = source.slice(start, end) + "}, false);";
    for (const event of [{type:"operator_ref"}, {type:"operator_lease",command:"acquire"}, {type:"operator_lease",command:"release"}]) {
      let now = 0, effects = 0;
      const page = {}, res = {destroyed:false};
      const action = async (_page, _event, current) => {
        if (!current()) throw new Error("retired request");
        effects++;return {};
      };
      const context = vm.createContext({performance:{now:()=>now},page,pageId:"page",pages:new Map([["page",page]]),res,body:{event},
        httpJson:()=>{},browserOperatorLease:action,browserRefInput:action,
        withBrowserInputWriter:async (_page,_current,callback)=>{
          if(boundary === "deadline") now = 1801;else res.destroyed = true;
          return callback();
        }});
      const result = vm.runInContext(`(async()=>{${body}})()`,context);
      if(event.command === "release") {await result;assert.equal(effects,1);}
      else {await assert.rejects(result,/retired request/);assert.equal(effects,0);}
    }
  });
}

for (const action of ["type", "click"]) {
  test(`actual CDP wrapper never replays ${action} after effect applied and ACK lost`, async () => {
    const h=harness({actualWrapper:true});let failed=false;
    try {
      await h.acquire();
      h.hook(async(method,params)=>{
        if(!failed && (action === "type" ? method === "Input.insertText" : method === "Input.dispatchMouseEvent" && params.type === "mousePressed")) {
          failed=true;
          throw new Error("browser CDP WebSocket closed");
        }
      });
      await assert.rejects(h.input(h.event(action)));
      const applied=effects(h).filter(c=>action === "type" ? c.method === "Input.insertText" : c.params.type === "mousePressed");
      assert.equal(applied.length,1,"native effect may have applied before connection failure");
    } finally {h.close();}
  });
}


test("actual CDP wrapper may connect before the first operator effect", async () => {
  const h=harness({actualWrapper:true});
  try {
    await h.acquire();h.page.browserPage._cdp.close();
    assert.equal((await h.input(h.event("type"))).accepted,true);
    assert.equal(effects(h).filter(c=>c.method === "Input.insertText").length,1);
  } finally {h.close();}
});
