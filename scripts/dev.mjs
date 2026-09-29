// One-command V3 development instance. No legacy database or production state.
import {existsSync,readFileSync,readdirSync,writeFileSync} from 'node:fs';
import {dirname,join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {bootstrapAdmin,composeArguments,dockerCommand,localDocker,localRequest} from './local-dev-runtime.mjs';
import {assertSameEngine,choosePort,composeOverride,createState,ensureExactFile,lockState,parsePrivateJson,privateDirectory,readState,regularFile,secretFiles,stateDirectory,writeNewState} from './local-dev-state.mjs';

const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const args=process.argv.slice(2);
if(args.includes('--help')){
  console.log('npm run dev [-- --no-build] : start persistent local HTTPS server + Web with Docker\nnpm run dev:down : stop without deleting data\nnpm run dev:info : show URL and credential/CA file paths\nOptional: LAYRS_DEV_INSTANCE (default), LAYRS_DEV_PORT (first start only).');
}else{
  let release;
  try{
    if(args.some(arg=>!['--no-build','--down','--info'].includes(arg))||args.filter(arg=>['--down','--info'].includes(arg)).length>1)throw new Error('Unknown dev arguments; use npm run dev -- --help');
    const directory=stateDirectory(root,process.env.LAYRS_DEV_INSTANCE);
    const stopping=args.includes('--down'),info=args.includes('--info');
    if((stopping||info)&&!existsSync(directory)){console.log('No local V3 development instance has been created.');process.exit(0);}
    privateDirectory(directory);release=lockState(directory);
    let state=readState(directory);
    if(!state&&(stopping||info))throw new Error('Local development manifest is missing; preserve the directory for recovery');
    const engine=info?null:localDocker(process.env);
    if(!state){
      if(readdirSync(directory).some(name=>name!=='command.lock'))throw new Error('Incomplete local development state; preserve the directory and investigate before retrying');
      state=createState(await choosePort(process.env.LAYRS_DEV_PORT),{engineId:engine.engineId,endpoint:engine.endpoint});writeNewState(directory,state);
    }
    if(engine)assertSameEngine(state,engine);
    if(process.env.LAYRS_DEV_PORT!==undefined&&Number(process.env.LAYRS_DEV_PORT)!==state.port)throw new Error('LAYRS_DEV_PORT differs from the saved instance. Use its existing port or a new LAYRS_DEV_INSTANCE.');
    const origin=`https://localhost:${state.port}`;
    const docker=(command,options)=>dockerCommand(root,directory,process.env,state,engine?.prefix??[],command,options);
    const compose=(...command)=>composeArguments(root,directory,state,...command);
    if(!info){
      const caddy=join(directory,'Caddyfile');
      const configuration=readFileSync(join(root,'deploy/selfhost/Caddyfile'),'utf8').replace('{$LAYRS_HOST} {','https://localhost {\n    tls internal');
      if(!configuration.includes('tls internal'))throw new Error('Local TLS template is missing its site marker');
      for(const [name,value] of Object.entries(secretFiles(state)))ensureExactFile(join(directory,name),value+'\n',0o644);
      ensureExactFile(join(directory,'compose.env'),'# Local dev: ambient .env files are intentionally not loaded.\n');
      // Derived routing may change with source code; credentials and identity may not.
      for(const [file,value] of [[caddy,configuration],[join(directory,'override.yml'),composeOverride(directory,state,caddy)]]){
        if(existsSync(file))regularFile(file);writeFileSync(file,value,{mode:0o600});
      }
      ensureExactFile(join(directory,'login.json'),JSON.stringify({origin,login:state.login,password:state.secrets.admin_password},null,2)+'\n');
    }
    if(stopping){
      console.log('Stopping only this local V3 instance; project data will be retained.');
      await docker(compose('down'));console.log('Stopped. Run npm run dev to reopen the same instance.');
    }else{
      if(!info){
        console.log(`Starting ${origin} (Docker build on first launch; this can take several minutes).`);
        console.log(`Private build/runtime diagnostic: ${join(directory,'docker.log')}`);
        const timer=setInterval(()=>console.log('Local dev startup is still running; Docker diagnostic is updated above.'),30000);
        try{
          await docker(compose('config','--quiet'));
          if(existsSync(join(directory,'bootstrapped.json')))await docker(['volume','inspect',...['postgres','cas','caddy_data','caddy_config'].map(name=>`${state.project}_${name}`)]);
          if(!args.includes('--no-build')){
            for(const [image,file] of [['layrs-server','Dockerfile'],['layrs-studio','Web.Dockerfile']])
              await docker(['buildx','build','--builder',engine.builder,'--load','-t',`${image}:${state.project}`,'-f',join(root,'deploy/selfhost',file),root]);
          }
          await docker(compose('up','-d','--no-build','--wait','--wait-timeout','180'));
          const proxy=await docker(compose('ps','-q','proxy'),{capture:true});
          if(!/^[0-9a-f]{12,64}$/.test(proxy))throw new Error('Cannot identify the local HTTPS proxy');
          const ca=await docker(['exec',proxy,'cat','/data/caddy/pki/authorities/local/root.crt'],{capture:true});
          if(!ca.includes('-----BEGIN CERTIFICATE-----'))throw new Error('Local HTTPS CA is unavailable');
          const caPath=join(directory,'root.crt');if(existsSync(caPath))regularFile(caPath);
          writeFileSync(caPath,ca+'\n',{mode:0o600});
          const ready=await localRequest(origin,ca,'GET','/readyz');
          if(ready.status!==200||parsePrivateJson(ready.text).ready!==true)throw new Error('Local server is not ready');
          await bootstrapAdmin(directory,state,ca);
        }finally{clearInterval(timer);}
      }
      console.log(`\nLayrs Web + API: ${origin}\nLogin: ${state.login}\nLocal password file: ${join(directory,'login.json')}\nDesktop CA file: ${join(directory,'root.crt')}\n`);
      console.log('The browser will flag this local certificate on first access. Inspect/trust this local CA explicitly; no system trust was changed.');
      console.log('Services run in the background. Stop with npm run dev:down; data is preserved.');
    }
  }catch(error){console.error(`Layrs dev: ${error.message}`);process.exitCode=1;}
  finally{release?.();}
}
