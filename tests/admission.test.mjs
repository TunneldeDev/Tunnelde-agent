import test from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { randomUUID } from "node:crypto";
import { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync, appendFileSync, rmSync, linkSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as pause } from "node:timers/promises";

const here = dirname(fileURLToPath(import.meta.url));
const helper = process.env.TUNNELDE_TEST_HELPER || resolve(here, "../target/debug", process.platform === "win32" ? "tunnelde-agent.exe" : "tunnelde-agent");
const packet = JSON.stringify({schemaVersion:1,configuration:{private:"initialized-private-configuration"},secrets:{token:"initialized-private-token"}});
function fixture() {
  const root=mkdtempSync(join(tmpdir(),"tunnelde-admission-")), id="task", token=randomUUID();
  const args=(operation, extra=[])=>[operation,"--directory",root,"--id",id,"--admission",token,...extra];
  const start=()=>args("start",["--bootstrap","initialize-fenced-v1","--executable",process.execPath,"--entry",join(here,"initialized-plugin.mjs")]);
  const run=(operation,input,extra=[])=>spawnSync(helper,operation==="start"?start():args(operation,extra),{input,encoding:"utf8",windowsHide:true,timeout:15000});
  const call=operation=>{const value=run(operation);assert.equal(value.status,0,value.stderr||value.error?.message);return JSON.parse(value.stdout)};
  const journal=()=>join(root,".admission-v1",id+".jsonl");
  const clean=()=>rmSync(root,{recursive:true,force:true});
  return {root,id,token,args,start,run,call,journal,clean};
}
const wait=async predicate=>{for(let n=0;n<150;n++){const value=predicate();if(value)return value;await pause(50)}assert.fail("Runner did not settle")};
const asyncRun=(args,input)=>new Promise((resolve,reject)=>{
  const child=spawn(helper,args,{windowsHide:true,stdio:["pipe","pipe","pipe"]});let stdout="",stderr="";
  child.stdout.on("data",value=>stdout+=value);child.stderr.on("data",value=>stderr+=value);
  child.once("error",reject);child.once("close",code=>resolve({code,stdout,stderr}));child.stdin.end(input);
});

test("cancellation before reservation is durable and late reservation/start cannot reopen it",()=>{
  const f=fixture();try{
    const cancelled=f.call("stop-fenced");assert.deepEqual(cancelled,{schemaVersion:1,runnerId:f.id,token:f.token,admission:"cancelled",status:"notFound"});
    const original=readFileSync(f.journal());
    for(let n=0;n<3;n++){
      assert.equal(f.call("reserve-fenced").admission,"cancelled");
      assert.notEqual(f.run("start",packet).status,0);
      assert.deepEqual(f.call("status-fenced"),cancelled);
      assert.deepEqual(readFileSync(f.journal()),original);
    }
    assert.deepEqual(readdirSync(f.root),[".admission-v1"]);
  }finally{f.clean()}
});

test("cancellation while start waits for EOF wins without creating the task",{timeout:6000},async()=>{
  const f=fixture();f.call("reserve-fenced");const child=spawn(helper,f.start(),{windowsHide:true,stdio:["pipe","pipe","pipe"]});
  let stderr="";child.stdout.resume();child.stderr.on("data",value=>stderr+=value);const done=once(child,"close");
  try{
    child.stdin.write(packet);await pause(200);assert.equal(child.exitCode,null);
    assert.equal(f.call("stop-fenced").admission,"cancelled");child.stdin.end();
    assert.notEqual((await done)[0],0);assert.match(stderr,/cancelled/);assert(!stderr.includes("initialized-private"));
    assert(!existsSync(join(f.root,f.id)));assert.equal(f.call("status-fenced").status,"notFound");
  }finally{if(child.exitCode===null){child.kill();await done}f.clean()}
});

test("start requires existing matching reservation and never recreates missing control data",()=>{
  const f=fixture();try{
    assert.notEqual(f.run("start",packet).status,0);assert.deepEqual(readdirSync(f.root),[]);
    f.call("reserve-fenced");const before=readFileSync(f.journal());
    const mismatch=spawnSync(helper,f.start().map(value=>value===f.token?randomUUID():value),{input:packet,encoding:"utf8",windowsHide:true});
    assert.notEqual(mismatch.status,0);assert.deepEqual(readFileSync(f.journal()),before);
    rmSync(f.journal());assert.notEqual(f.run("start",packet).status,0);assert(!existsSync(f.journal()));
    assert.equal(f.call("stop-fenced").admission,"cancelled");assert.notEqual(f.run("start",packet).status,0);
  }finally{f.clean()}
});

test("independent reserve and cancel processes serialize and cancellation never loses",{timeout:15000},async()=>{
  const f=fixture();try{
    const values=await Promise.all(Array.from({length:16},(_,n)=>asyncRun(f.args(n%2?"reserve-fenced":"stop-fenced"))));
    for(const value of values)assert.equal(value.code,0,value.stderr);
    assert.equal(f.call("status-fenced").admission,"cancelled");assert.notEqual(f.run("start",packet).status,0);
    assert(readFileSync(f.journal(),"utf8").split("\n").length<=3);assert(!existsSync(join(f.root,f.id)));
  }finally{f.clean()}
});

test("a real fenced launch is single-use and cross-process cancellation stops its child",{timeout:20000},async()=>{
  const f=fixture();let launched=false;try{
    assert.equal(f.call("reserve-fenced").admission,"reserved");assert.equal(f.run("start",packet).status,0);launched=true;
    assert.notEqual(f.run("start",packet).status,0);assert.equal(f.call("status-fenced").admission,"launched");
    await wait(()=>{
      const result=spawnSync(helper,["poll","--directory",f.root,"--id",f.id],{encoding:"utf8",windowsHide:true});
      assert.equal(result.status,0,result.stderr);return JSON.parse(result.stdout).events.some(event=>event.message.event==="ready");
    });
    // Simulate a supervisor/control crash after persisting cancellation but
    // before STOP. A subsequent helper process must still deliver STOP.
    appendFileSync(f.journal(),JSON.stringify({schemaVersion:1,runnerId:f.id,token:f.token,admission:"cancelled"})+"\n");
    assert.equal(f.call("status-fenced").admission,"cancelled");
    assert(!["stopped","failed","notFound"].includes(f.call("status-fenced").status));
    assert.equal(f.call("stop-fenced").admission,"cancelled");
    await wait(()=>["stopped","failed"].includes(f.call("status-fenced").status));
    assert.notEqual(f.run("start",packet).status,0);
    assert(!readFileSync(f.journal(),"utf8").includes("initialized-private"));
    assert(readFileSync(f.journal()).length<=1024);
  }finally{if(launched){f.run("stop-fenced");await wait(()=>["stopped","failed"].includes(f.call("status-fenced").status))}f.clean()}
});

test("racing launch/stop never leaves admission open or permits a replay",{timeout:20000},async()=>{
  const f=fixture();try{
    f.call("reserve-fenced");const [start,stop]=await Promise.all([asyncRun(f.start(),packet),asyncRun(f.args("stop-fenced"))]);
    assert.equal(stop.code,0,stop.stderr);assert([0,1].includes(start.code));
    await wait(()=>["stopped","failed","notFound"].includes(f.call("status-fenced").status));
    assert.equal(f.call("status-fenced").admission,"cancelled");assert.notEqual(f.run("start",packet).status,0);
  }finally{f.run("stop-fenced");f.clean()}
});

test("malformed, partial, oversized and wrong-token journals remain intact and fail closed",()=>{
  const f=fixture();try{
    f.call("reserve-fenced");const valid=readFileSync(f.journal(),"utf8");
    for(const bytes of [valid.trimEnd(),valid+"{",valid+valid,"x".repeat(1025),valid.replace(f.token,randomUUID()),valid.replace('"schemaVersion":1','"schemaVersion":2')]){
      writeFileSync(f.journal(),bytes);
      for(const operation of ["reserve-fenced","status-fenced","stop-fenced","start"]){
        const result=f.run(operation,operation==="start"?packet:undefined);assert.notEqual(result.status,0);assert(!result.stdout);assert.equal(readFileSync(f.journal(),"utf8"),bytes);
      }
      assert(!existsSync(join(f.root,f.id)));
    }
  }finally{f.clean()}
});

test("capacity refuses new reservations but keeps existing cancellation available",{timeout:15000},()=>{
  const f=fixture();try{
    f.call("reserve-fenced");for(let n=0;n<4095;n++)writeFileSync(join(f.root,".admission-v1",`other-${n}.jsonl`),"",{mode:0o600});
    const result=spawnSync(helper,f.args("reserve-fenced").map(value=>value===f.id?"new-task":value),{encoding:"utf8",windowsHide:true});
    assert.notEqual(result.status,0);assert.match(result.stderr,/capacity/);assert.equal(readdirSync(join(f.root,".admission-v1")).length,4097);
    assert.equal(f.call("stop-fenced").admission,"cancelled");
  }finally{f.clean()}
});

test("hardlinked journals and redirected admission directories cannot mutate their targets",()=>{
  const f=fixture();try{
    f.call("reserve-fenced");const target=join(f.root,"keep.txt");writeFileSync(target,"keep",{mode:0o600});
    rmSync(f.journal());linkSync(target,f.journal());assert.notEqual(f.run("stop-fenced").status,0);assert.equal(readFileSync(target,"utf8"),"keep");
    rmSync(join(f.root,".admission-v1"),{recursive:true});
    const outside=mkdtempSync(join(tmpdir(),"tunnelde-admission-redirect-"));
    try{symlinkSync(outside,join(f.root,".admission-v1"),process.platform==="win32"?"junction":"dir");assert.notEqual(f.run("stop-fenced").status,0);assert.deepEqual(readdirSync(outside),[])}finally{rmSync(join(f.root,".admission-v1"),{recursive:true});rmSync(outside,{recursive:true})}
  }finally{f.clean()}
});
