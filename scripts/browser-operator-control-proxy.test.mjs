import assert from "node:assert/strict";
import {mkdtempSync,readFileSync,rmSync} from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
const source=readFileSync(new URL("./browser-vm-control-service.mjs",import.meta.url),"utf8");
function declaration(name){const start=source.search(new RegExp(`(?:async )?function ${name}\\(`));const end=source.slice(start).search(/\n\}(?=\n|$)/);assert.ok(start>=0&&end>0);return source.slice(start,start+end+2);}
async function harness(t){
  const root=mkdtempSync(path.join(os.tmpdir(),"browser-operator-")),socket=path.join(root,"guest.sock");
  let respond=(_req,res)=>res.end(JSON.stringify({accepted:true,page_id:"page:owned"}));const requests=[];
  const guest=http.createServer(async(req,res)=>{let body="";for await(const chunk of req)body+=chunk;requests.push({path:req.url,body:JSON.parse(body)});respond(req,res);});
  const activePages=new Map([["page:owned",{page:{control_socket_path:socket}}]]),activeVms=new Map();
  const context=vm.createContext({http,Buffer,Error,AbortController,setTimeout,clearTimeout});
  vm.runInContext(["safeId","validateAbsolutePath","vmOwnsExactPage", "ownedVmRecord", "activePageGuestControl","requestJsonOverUnix","browserDisplayControlError","browserInspectionControlError","proxyGuestPageInput"].map(declaration).join("\n"),context);
  t.after(async()=>{guest.closeAllConnections();if(guest.listening)await new Promise(resolve=>guest.close(resolve));rmSync(root,{recursive:true,force:true});});
  await new Promise(resolve=>guest.listen(socket,resolve));
  return {activePages,requests,respond:fn=>{respond=fn;},input:()=>context.proxyGuestPageInput({},activePages,activeVms,"page:owned",{event:{type:"operator_ref"}})};
}
test("operator input uses exact owned Unix page and rejects retired ownership before dispatch",async t=>{
  const h=await harness(t);assert.equal((await h.input()).accepted,true);
  assert.equal(h.requests[0].path,"/pages/page%3Aowned/input");h.activePages.get("page:owned").cleanup_pending=true;
  await assert.rejects(h.input());assert.equal(h.requests.length,1);
});
test("operator proxy rejects replacement during response and bounds reply bytes",async t=>{
  const h=await harness(t);const owner=h.activePages.get("page:owned");
  h.respond((_req,res)=>{h.activePages.set("page:owned",{...owner});res.end('{"accepted":true}');});
  await assert.rejects(h.input(),/ownership changed/);
  h.respond((_req,res)=>res.end(JSON.stringify({padding:"x".repeat(4097)})));
  await assert.rejects(h.input());
});
test("operator proxy timeout closes its own hanging Unix exchange",async t=>{
  const h=await harness(t);h.respond(()=>{});const started=performance.now();
  await assert.rejects(h.input());assert.ok(performance.now()-started<2500);
});
test("input route logs only allowlisted fields for failed input and resize outcomes",async t=>{
  const h=await harness(t),events=[];
  const routeStart=source.indexOf("      const pageInputMatch ="),routeEnd=source.indexOf("      const pageWebrtcMatch =",routeStart);
  assert.ok(routeStart>0&&routeEnd>routeStart);
  const context=vm.createContext({http,Buffer,Error,Date,AbortController,setTimeout,clearTimeout,config:{signal_timeout_ms:2000},
    activePages:h.activePages,activeVms:new Map(),MAX_BROWSER_INPUT_BODY_BYTES:1<<20,logEvent:(event,fields)=>events.push({event,...JSON.parse(JSON.stringify(fields))})});
  vm.runInContext(["safeId","validateAbsolutePath","vmOwnsExactPage","ownedVmRecord","activePageGuestControl","requestJsonOverUnix","browserDisplayControlError","browserInspectionControlError","postJsonOverUnix","proxyGuestPageInput","readJsonBody","pageInputErrorCode","logPageInput"].map(declaration).join("\n"),context);
  const handler=vm.runInContext(`(async function(req, url, sendJson) { ${source.slice(routeStart,routeEnd)}\n sendJson(405, {}); })`,context);
  const proxy=http.createServer((req,res)=>{handler(req,new URL(req.url,"http://localhost"),(status,body)=>{res.writeHead(status,{"content-type":"application/json"});res.end(JSON.stringify(body));});});
  t.after(async()=>{proxy.closeAllConnections();await new Promise(resolve=>proxy.close(resolve));});
  await new Promise(resolve=>proxy.listen(0,"127.0.0.1",resolve));
  const endpoint=page=>`http://127.0.0.1:${proxy.address().port}/pages/${page}/input`;
  const post=(event,page="page%3Aowned")=>fetch(endpoint(page),{method:"POST",body:JSON.stringify({event})});
  const resize={type:"resize",width:390,height:1200,sequence:7},secret="https://secret.invalid/?token=tok_SECRET";
  const guestError=(status,error)=>h.respond((_req,res)=>{res.writeHead(status,{"content-type":"application/json"});res.end(JSON.stringify({error,code:"tok_SECRET"}));});
  guestError(500,`resize failed at ${secret}`);assert.equal((await post(resize)).status,404);
  guestError(500,`navigate failed: ${secret}`);
  assert.equal((await post({type:"browser_command",command:"navigate",url:secret})).status,404);
  assert.equal((await post({type:secret})).status,404);
  assert.equal((await fetch(endpoint("page%3Aowned"),{method:"POST",body:'{"event":{"type":"paste_text","text":"tok_SECRET'})).status,404);
  assert.equal((await post(resize,"page%3Amissing")).status,404);
  h.respond((_req,res)=>res.end(JSON.stringify({schema:"elastos.browser.input-result/v1",page_id:"page:owned",accepted:true,width:390,height:1200,sequence:7})));
  assert.equal((await post(resize)).status,200);
  h.respond((_req,res)=>res.end(JSON.stringify({schema:"elastos.browser.input-result/v1",page_id:"page:owned",accepted:false,resize_error:{code:"tok_SECRET",message:secret}})));
  assert.equal((await post(resize)).status,200);
  assert.equal((await post({type:"paste_text",text:"tok_SECRET private words"})).status,200);
  const logged=JSON.stringify(events);
  assert.ok(!logged.includes("SECRET")&&!logged.includes("secret")&&!logged.includes("private words"),logged);
  assert.deepEqual(events.map(({event,event_type,code,guest_status})=>[event,event_type,code??null,guest_status??null]),[
    ["page_input_failed","resize","guest_rejected",500],["page_input_failed","browser_command","guest_rejected",500],
    ["page_input_failed","other","guest_rejected",500],["page_input_failed","other","invalid_request",null],
    ["page_input_failed","resize","page_not_found",null],["page_input_resize","resize",null,null],["page_input_resize","resize",null,null]]);
  assert.equal(events[0].page_id,"page:owned");assert.deepEqual(events[0].requested,{width:390,height:1200,sequence:7});
  assert.equal(events[4].page_id,"page:missing");
  const [accepted,refused]=events.slice(5);
  assert.deepEqual([accepted.schema_match,accepted.page_id_match,accepted.accepted,accepted.width,accepted.height,accepted.sequence,accepted.resize_error],[true,true,true,390,1200,7,null]);
  assert.deepEqual([refused.accepted,refused.resize_error],[false,"other"]);
});
