import test from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, existsSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as pause } from "node:timers/promises";

const directory = dirname(fileURLToPath(import.meta.url));
const helper = process.env.TUNNELDE_TEST_HELPER || resolve(directory,"../target/debug",process.platform === "win32" ? "tunnelde-agent.exe" : "tunnelde-agent");
const packet = { schemaVersion:1, configuration:{ private:"initialized-private-configuration" },secrets:{token:"initialized-private-token"} };
const run = (args,input) => spawnSync(helper,args,{input,encoding:"utf8",windowsHide:true,timeout:15000});
const argumentsFor = root => ["start","--directory",root,"--id","initialized","--bootstrap","initialize-v1","--executable",process.execPath,"--entry",join(directory,"initialized-plugin.mjs")];

test("a valid prefix cannot launch before EOF or conceal a trailing second document",{timeout:5000},async()=>{
  const root=mkdtempSync(join(tmpdir(),"tunnelde-bootstrap-partial-"));
  const child=spawn(helper,argumentsFor(root),{windowsHide:true,stdio:["pipe","pipe","pipe"]});
  let stderr="";
  child.stdout.resume();child.stderr.on("data",chunk=>stderr+=chunk);
  const closed=once(child,"close");
  try {
    child.stdin.write(JSON.stringify(packet));
    await pause(250);
    assert.equal(child.exitCode,null);
    assert.deepEqual(readdirSync(root),[]);
    child.stdin.end("\n{}\n");
    const [code]=await closed;
    assert.notEqual(code,0);
    assert(!stderr.includes("initialized-private"));
    assert.deepEqual(readdirSync(root),[]);
  } finally {
    if(child.exitCode===null){child.kill();await closed}
    rmSync(root,{recursive:true,force:true});
  }
});

test("invalid initialized input never creates task artifacts or echoes payloads",()=>{
  const root=mkdtempSync(join(tmpdir(),"tunnelde-bootstrap-invalid-"));
  try {
    for (const input of ["",'{"schemaVersion":1,',JSON.stringify({...packet,schemaVersion:"initialized-private-token"}),JSON.stringify(packet)+"\n{}",JSON.stringify({...packet,configuration:{private:"x".repeat(200*1024)}})]) {
      const result=run(argumentsFor(root),input);
      assert.notEqual(result.status,0);
      assert(!result.stdout);
      assert(!result.stderr.includes("initialized-private"));
      assert.deepEqual(readdirSync(root),[]);
    }
    const result=run([...argumentsFor(root),"--config","private-path"],JSON.stringify(packet));
    assert.notEqual(result.status,0);
    assert.deepEqual(readdirSync(root),[]);
  } finally {rmSync(root,{recursive:true,force:true})}
});

test("initialized input times out without EOF and leaves no task directory", {timeout:15000}, async()=>{
  const root=mkdtempSync(join(tmpdir(),"tunnelde-bootstrap-timeout-"));
  const child=spawn(helper,argumentsFor(root),{windowsHide:true,stdio:["pipe","pipe","pipe"]});
  const closed=once(child,"close");
  let stderr="";
  child.stdout.resume();
  child.stderr.on("data",chunk=>stderr+=chunk);
  try {
    child.stdin.write(JSON.stringify(packet));
    const [code]=await closed;
    assert.notEqual(code,0);
    assert.match(stderr,/input timed out/);
    assert(!stderr.includes("initialized-private"));
    assert.deepEqual(readdirSync(root),[]);
  } finally {
    if(child.exitCode===null){child.kill();await closed}
    child.stdin.destroy();
    rmSync(root,{recursive:true,force:true});
  }
});

test("initialized supervisor passes config only through stdin and never seeds durable storage",{timeout:30000},async()=>{
  const root=mkdtempSync(join(tmpdir(),"tunnelde-bootstrap-"));
  const control=command=>{
    const result=run([command,"--directory",root,"--id","initialized"]);
    assert.equal(result.status,0,result.stderr || result.error?.message);
    return JSON.parse(result.stdout);
  };
  const wait=async predicate=>{for(let n=0;n<100;n++){const value=predicate();if(value)return value;await pause(70)}assert.fail("Supervisor did not settle")};
  try {
    const started=run(argumentsFor(root),JSON.stringify(packet)+"\n");
    assert.equal(started.status,0,started.stderr || started.error?.message);
    assert.equal(JSON.parse(started.stdout).status,"starting");
    await wait(()=>control("poll").events.find(item=>item.message.event==="ready"));
    const sent=run(["send","--directory",root,"--id","initialized"],JSON.stringify({jsonrpc:"2.0",id:1,method:"command.run",params:{input:{}}})+"\n");
    assert.equal(sent.status,0,sent.stderr);
    const response=await wait(()=>control("poll").events.find(item=>item.message.type==="response"&&item.message.message.id===1)?.message.message);
    assert.deepEqual(response.result,{configurationReceived:true,secretReceived:true,hostSecretReceived:true,configurationSeed:null,argv:["--initialize-config"]});
    const storage=JSON.parse(readFileSync(join(root,"initialized/storage.json"),"utf8"));
    assert.deepEqual(storage,{ordinary:"allowed persistence"});
    control("stop");
    await wait(()=>control("status").status==="stopped");
    const inspect=path=>{for(const entry of readdirSync(path,{withFileTypes:true})){const child=join(path,entry.name);if(entry.isDirectory())inspect(child);else{const bytes=readFileSync(child);assert(!bytes.includes(Buffer.from("initialized-private-configuration")),child);assert(!bytes.includes(Buffer.from("initialized-private-token")),child)}}};
    inspect(root);
    assert(!existsSync(join(root,"config-initialized.json")));
  } finally {
    if(existsSync(join(root,"initialized/pid"))) {try {control("stop");await wait(()=>["stopped","failed"].includes(control("status").status))}catch{}}
    rmSync(root,{recursive:true,force:true});
  }
});
