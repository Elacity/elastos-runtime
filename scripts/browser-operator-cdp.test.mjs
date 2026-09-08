import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import vm from "node:vm";
import { spawn } from "node:child_process";
import { once } from "node:events";
import readline from "node:readline";
import test from "node:test";

const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");
const block = (start, end) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
const python = String.raw`
import importlib.util,json,sys
from types import SimpleNamespace
spec=importlib.util.spec_from_file_location("gate",sys.argv[1]); module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
events=[]; clock=[1.0]
native=SimpleNamespace(on_message=events.append,send_x11_keypress=lambda *a,**k:None,send_x11_mouse=lambda *a,**k:None,xdisplay=SimpleNamespace(sync=lambda:None))
gate=module.InputWriterGate(native,sys.argv[2],lambda:clock[0])
print(json.dumps({"ready":True}),flush=True)
try:
 for line in sys.stdin:
  request=json.loads(line)
  if "time" in request:clock[0]=request["time"]
  if "message" in request:gate.on_message(request["message"])
  print(json.dumps({"events":events,"held":gate.lease is not None}),flush=True)
finally:gate.close()
`;

async function harness(action, lostConnection, testContext) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "browser-cdp-gate-"));
  const socketPath = path.join(directory, "writer.sock");
  const child = spawn("python3", ["-u", "-c", python,
    new URL("./browser-input-writer-gate.py", import.meta.url).pathname, socketPath], {stdio:["pipe","pipe","pipe"]});
  let ownedClient=null, cleaned=false;
  const cleanup=async()=>{
    if(cleaned)return;cleaned=true;ownedClient?.close();
    const exited=child.exitCode !== null || child.signalCode !== null ? Promise.resolve() : once(child,"exit");
    child.stdin.end();await exited;fs.rmSync(directory,{recursive:true,force:true});
  };
  testContext.after(cleanup);
  const lines = readline.createInterface({input:child.stdout})[Symbol.asyncIterator]();
  assert.equal(JSON.parse((await lines.next()).value).ready,true);
  const native = async request => {child.stdin.write(JSON.stringify(request)+"\n");return JSON.parse((await lines.next()).value);};
  let now=10, expiry, serial=0;
  const applied=[], replies=[];
  const frame={id:"frame",loaderId:"loader",url:"https://controlled.test/form"};
  class Socket {
    constructor(){this.serial=++serial;this.text=()=>{};this.closed=false;}
    async connect(){}
    onText(fn){this.text=fn;}
    onError(fn){this.error=fn;}
    onClose(fn){this.end=fn;}
    close(){if(!this.closed){this.closed=true;this.end?.();}}
    sendText(raw){
      const message=JSON.parse(raw), {id,method,params}=message;
      let result={};
      if(method==="Page.getFrameTree")result={frameTree:{frame}};
      if(method==="DOM.describeNode")result={node:{backendNodeId:7,nodeName:"INPUT",attributes:["type","text"]}};
      if(method==="DOM.getContentQuads")result={quads:[[10,10,100,10,100,40,10,40]]};
      if(method==="DOM.getNodeForLocation")result={backendNodeId:7};
      if(method==="DOM.getDocument")result={root:{nodeId:1}};
      if(method==="DOM.querySelector")result={nodeId:7};
      if(method==="Input.insertText" || method==="Input.dispatchMouseEvent")applied.push(message);
      const target=action==="type" ? method==="Input.insertText" : method==="Input.dispatchMouseEvent"&&params.type==="mousePressed";
      if(target){
        // Apply the simulated native effect first; transport ACK comes later or
        // is lost. Actual CdpClient pending/timeout/close handling remains active.
        if(lostConnection)queueMicrotask(()=>this.close());
        else replies.push(()=>this.text(JSON.stringify({id,result})));
      }else queueMicrotask(()=>this.text(JSON.stringify({id,result})));
    }
  }
  const context=vm.createContext({console,Buffer,URL,performance:{now:()=>now},
    net:{createConnection(options){assert.equal(options.path,"/run/elastos/browser-input-writer.sock");return net.createConnection({path:socketPath});}},
    setTimeout(fn,ms){if(ms>29000){expiry=fn;return {unref(){}};}return setTimeout(fn,ms);},
    clearTimeout,
    MinimalWebSocketClient:Socket,validatePasteText:value=>value,summarizeBrowserFileChooser:()=>null,
    ensureBrowserFileChooserInterception:()=>{throw new Error("operator input must reuse owned CDP");}});
  vm.runInContext(block('function withTimeout(', '\nfunction createBrowserFileChooserState')+
    block('class CdpClient {','\nasync function fetchBrowserControlJson')+
    block('const OPERATOR_ID =','\nfunction closeCachedBrowserCdp')+
    block('function finiteCoordinate(', '\nfunction validateBrowserNavigationUrl')+
    block('async function pasteTextIntoBrowserPage(', '\nasync function browserPageStateFromCdp')+
    '\nglobalThis.RealCdpClient=CdpClient;',context);
  const client=new context.RealCdpClient("ws://private-cdp/page",15000);ownedClient=client;await client.connect(1000);
  const snapshot={id:"b".repeat(32),expires:30010,backendNodes:[{backendDOMNodeId:7,frameId:"frame"}]};
  const page={pageId:"page:owned",closed:false,browserPage:{debugger_url:"ws://private-cdp/page",_cdp:client,width:1920,height:1080,
    _inspection:{generation:"a".repeat(32),snapshot,binding:"frame:loader:https://controlled.test/form"}}};
  const lease={command:"acquire",admission_id:"c".repeat(32),document_generation:"a".repeat(32),actions:["click","type"],duration_ms:30000};
  await context.browserOperatorLease(page,lease,()=>true);
  const request=client.request.bind(client);
  client.request=(method,params,timeout)=>{
    // Use the actual public deadline with only 10ms remaining at first effect.
    if(method==="DOM.getNodeForLocation" || method==="DOM.querySelector")now=1500;
    return request(method,params,timeout);
  };
  const event={schema:"elastos.browser.ref-input/v1",request_id:"d".repeat(32),admission_id:lease.admission_id,
    document_generation:lease.document_generation,ref:`${snapshot.id}:0`,action,...(action==="type"?{text:"once"}:{})};
  return {context,page,applied,replies,native,event,lease,client,expire:()=>expiry(),
    run:()=>context.browserRefInput(page,event,()=>true),
    release:()=>context.browserOperatorLease(page,{command:"release",admission_id:lease.admission_id},()=>true),
    connections:()=>serial,
    async close(){page.closed=true;clearTimeout(page.operatorLease?.timer);await cleanup();}};
}

for(const action of ["type","click"]){
  test(`real CDP + Unix native gate: ${action} late ACK keeps human blocked across expiry and release`,async(t)=>{
    const h=await harness(action,false,t);
    try{
      await assert.rejects(h.run());
      assert.equal(h.replies.length,1);
      const applied=h.applied.filter(m=>action==="type"?m.method==="Input.insertText":m.params.type==="mousePressed");
      assert.equal(applied.length,1);
      if(action==="click")assert.deepEqual(h.applied.map(m=>m.params.type),["mouseMoved","mousePressed","mouseReleased"]);
      await h.native({time:32});h.expire();
      await assert.rejects(h.release());
      assert.deepEqual((await h.native({message:"kd,99"})).events,[]);
      h.replies[0]();
      const deadline=Date.now()+1000;
      while(h.page.operatorLease?.effect && Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,5));
      assert.equal(h.page.operatorLease?.effect ?? null,null,"late ACK must settle the exact native hold");
      assert.deepEqual((await h.native({message:"kd,100"})).events,["kd,100"]);
      assert.equal(h.connections(),1);
      await assert.rejects(h.run(),"uncertain request must never replay after late ACK");
      assert.equal(h.applied.filter(m=>action==="type"?m.method==="Input.insertText":m.params.type==="mousePressed").length,1);
    }finally{await h.close();}
  });
  test(`real CDP + Unix native gate: ${action} lost ACK never reconnects/replays or releases pending effect`,async(t)=>{
    const h=await harness(action,true,t);
    try{
      await assert.rejects(h.run());
      await h.native({time:100});h.expire();
      await assert.rejects(h.release());
      assert.deepEqual((await h.native({message:"kd,99"})).events,[]);
      assert.equal(h.connections(),1);
      assert.equal(h.applied.filter(m=>action==="type"?m.method==="Input.insertText":m.params.type==="mousePressed").length,1);
    }finally{await h.close();}
  });
}
