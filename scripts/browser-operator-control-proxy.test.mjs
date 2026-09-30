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
  vm.runInContext(["safeId","validateAbsolutePath","activePageGuestControl","requestJsonOverUnix","browserDisplayControlError","browserInspectionControlError","proxyGuestPageInput"].map(declaration).join("\n"),context);
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
