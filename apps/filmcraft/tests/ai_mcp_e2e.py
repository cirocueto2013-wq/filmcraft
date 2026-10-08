"""Real native + MCP + Kokoro + ComfyUI E2E. Requires running real services, no fake inference.

Start FilmCraft --empty --no-recover --control 9882; run with --cli /path/to/filmcraft-cli.
ComfyUI's native EmptyImage/SaveImage graph tests its real output path without requiring a checkpoint.
Kokoro speech is neural inference. FFmpeg/ffprobe are external verification tools only.
"""
import argparse, array, base64, json, math, os, pathlib, select, socket, subprocess, time, wave
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--port',type=int,default=9882)
p.add_argument('--cli',required=True)
p.add_argument('--out',type=pathlib.Path,required=True)
p.add_argument('--kokoro-url',default='http://127.0.0.1:8880')
p.add_argument('--comfy-url',default='http://127.0.0.1:8188')
p.add_argument('--require-audio-clock',action='store_true',help='Require playback through a physical or clocked virtual audio device')
p.add_argument('--playback-capture',type=pathlib.Path,help='Optional external PCM sink file; record byte boundaries for verifying the real callback')
p.add_argument('--ffmpeg',default=os.environ.get('FILMCRAFT_FFMPEG','ffmpeg'))
p.add_argument('--ffprobe',default=os.environ.get('FILMCRAFT_FFPROBE','ffprobe'))
a=p.parse_args();root=a.out.resolve();root.mkdir(parents=True,exist_ok=True);T=254016000000
trace=[]
log=open(root/'mcp-stderr.log','w')
proc=subprocess.Popen([a.cli,'mcp','--bridge',f'127.0.0.1:{a.port}'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True,bufsize=1)
seq=0

def rpc(method,params):
 global seq
 seq+=1;payload={'jsonrpc':'2.0','id':seq,'method':method,'params':params}
 proc.stdin.write(json.dumps(payload)+'\n');proc.stdin.flush()
 until=time.monotonic()+120
 while time.monotonic()<until:
  if not select.select([proc.stdout],[],[],1)[0]:continue
  line=proc.stdout.readline();assert line,'MCP process exited'
  response=json.loads(line)
  if response.get('id')==seq:
   trace.append({'request':payload,'response':response});(root/'mcp-trace.json').write_text(json.dumps(trace,indent=2))
   assert 'error' not in response,response
   return response['result']
 raise AssertionError('MCP response timed out')

def tool(name,**args):
 r=rpc('tools/call',{'name':name,'arguments':args});assert not r.get('isError'),r
 return json.loads(r['content'][0]['text'])

def ex(command,**params):return tool('command_run',id=command,params=params)
def sequence():
 r=ex('sequence.inspect');r.pop('selection',None);r.pop('playhead',None);return r

def wait_ai():
 until=time.monotonic()+120
 while time.monotonic()<until:
  state=tool('ai_job_status')
  if not state['busy']:
   assert not state['status'].get('error'),state
   return state
  time.sleep(.05)
 raise AssertionError('AI job timed out')

sock=socket.create_connection(('127.0.0.1',a.port),timeout=15);sock.settimeout(60);stream=sock.makefile('rwb')
def ui(method,**params):
 stream.write((json.dumps({'id':1,'method':method,'params':params})+'\n').encode());stream.flush()
 r=json.loads(stream.readline());assert r['ok'],r;return r.get('result')

def all_paths(v):
 if isinstance(v,dict):
  for key,value in v.items():
   if key=='path' and isinstance(value,str):yield pathlib.Path(value)
   yield from all_paths(value)
 elif isinstance(v,list):
  for value in v:yield from all_paths(value)

def decode_audio(path):
 b=subprocess.check_output([a.ffmpeg,'-v','error','-i',str(path),'-ac','1','-ar','48000','-f','f32le','-'])
 samples=array.array('f');samples.frombytes(b);return samples

def correlation(x,y):
 assert len(x)==len(y) and len(x)>1000
 xy=sum(a*b for a,b in zip(x,y));xx=sum(a*a for a in x);yy=sum(b*b for b in y)
 return xy/math.sqrt(max(xx*yy,1e-20))

try:
 rpc('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'filmcraft-real-ai-e2e','version':'1'}})
 proc.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n');proc.stdin.flush()
 names={t['name'] for t in rpc('tools/list',{})['tools']};assert {'ai_generate_voice','comfy_generate','assistant_preview','editor_autopilot'}<=names
 ex('file.newProject',name='AI and MCP verification');ex('file.newSequence',width=160,height=90,fps=24,video=1,audio=1)
 project=root/'ai-edit.fcproj';ex('file.saveAs',path=str(project))
 tool('ai_configure',kokoro_url=a.kokoro_url,comfy_url=a.comfy_url)
 tool('ai_voices');voices=wait_ai()['status']['result']['voices'];assert {'ef_dora','af_heart'}<={v['id'] for v in voices}
 tool('ai_generate_voice',text='Hola. Esta voz en español se genera con Kokoro para Filmcraft.',voice='ef_dora',language='es')
 state=wait_ai();first=tool('ai_import_media',token=state['ready']['token'],place=True,seconds=0)['items'][0]
 es_duration=sequence()['duration']/T;en_start=math.ceil(es_duration)+1
 tool('ai_generate_voice',text='Hello. This English narration is generated with Kokoro for Filmcraft.',voice='af_heart',language='en')
 state=wait_ai();second=tool('ai_import_media',token=state['ready']['token'],place=True,seconds=en_start)['items'][0]
 tool('comfy_connect');assert wait_ai()['status']['result']['system']
 workflow={'1':{'class_type':'EmptyImage','inputs':{'width':160,'height':90,'batch_size':1,'color':16711680}},'2':{'class_type':'SaveImage','inputs':{'images':['1',0],'filename_prefix':'Filmcraft_AI_E2E'}}}
 tool('comfy_generate',workflow=workflow);state=wait_ai();assert state['ready']['assets'][0]['name'].endswith('.png')
 total=sequence()['duration']/T;tool('ai_import_media',token=state['ready']['token'],place=True,seconds=0,duration_seconds=math.ceil(total))
 before=sequence();audio=before['audio'][0];clips=audio['items'];print('Imported clip metadata:',json.dumps(clips)[:2000],flush=True)
 es_clip=next(c['clip'] for c in clips if c['item']==first);en_clip=next(c['clip'] for c in clips if c['item']==second)
 history=ex('history.list')
 preview=tool('assistant_preview',actions=[{'type':'gain','clip':es_clip,'db':-3},{'type':'trim','clip':en_clip,'edge':'out','delta':-1},{'type':'split','clip':es_clip,'time':1.0}],local=True)
 assert sequence()==before and ex('history.list')==history,'preview changed the live project'
 example=tool('editor_training_example',instruction='Reduce Spanish narration by 3 dB, shorten English narration by one second and split Spanish at one second.')
 (root/'reviewed-example.jsonl').write_text(json.dumps(example)+'\n')
 tool('assistant_apply',token=preview['plan']['token']);edited=sequence();assert len(ex('history.list')['undo'])==len(history['undo'])+1
 ex('edit.undo');assert sequence()==before;ex('edit.redo');assert sequence()==edited
 # Move a clip after rendering/generation using a second validated plan and undo/redo.
 preview=tool('assistant_preview',actions=[{'type':'move','clip':en_clip,'start':en_start+.5,'track':audio['id']}],local=True)
 tool('assistant_apply',token=preview['plan']['token']);moved=sequence();ex('edit.undo');assert sequence()==edited;ex('edit.redo');assert sequence()==moved
 ui('ui.panel.show',panel='Text');ui('ui.click',id='text.tab.Assistant');time.sleep(.15)
 ui('ui.screenshot',path=str(root/'assistant-ui.png'))
 if not any(e['id']=='ai.apiKey' for e in ui('ui.elements',prefix='ai.')):
  ui('ui.click',id='ai.section.connections');time.sleep(.15)
 ui('ui.click',id='ai.apiKey');ui('ui.key',key='A',command=True,ctrl=True)
 ui('ui.type',text='verification-placeholder-not-a-credential');time.sleep(.1)
 assert 'verification-placeholder-not-a-credential' not in json.dumps(ui('ui.inspect'))
 ui('ui.screenshot',path=str(root/'cloud-settings-ui.png'))
 # Rapid seeks must settle on the actual generated image, then playback must advance with audio.
 for t in [4,0,.5,6,1,.1]:ex('playhead.set',seconds=t)
 ui('ui.screenshot',path=str(root/'generated-preview.png'),panel='Program')
 captured={'startByte':a.playback_capture.stat().st_size} if a.playback_capture else None
 start=ui('ui.inspect')['playhead'];started=ui('ui.playback',action='play');assert started['playing'],started
 time.sleep(.6);playing=ui('ui.inspect');ui('ui.playback',action='stop')
 assert playing['playhead']>start,playing
 if a.require_audio_clock:
  assert playing['playback']['playing'] and playing['playback']['audioClock'],playing
  assert playing['playback']['shown']>=2,playing
 if captured is not None:
  time.sleep(.1);captured.update(endByte=a.playback_capture.stat().st_size,startTick=start)
  assert captured['endByte']>captured['startByte'],captured
  (root/'playback-capture-boundaries.json').write_text(json.dumps(captured,indent=2))
 ex('file.save');saved=sequence();ex('file.closeProject');ex('file.open',path=str(project));assert sequence()==saved
 paths=list(all_paths(json.loads(project.read_text())));es_file=next(path for path in paths if path.name.endswith('ef_dora.wav'));en_file=next(path for path in paths if path.name.endswith('af_heart.wav'))
 assert es_file.exists() and en_file.exists()
 for item,source in [(first,es_file),(second,en_file)]:
  with wave.open(str(source)) as wav:
   expected=wav.getnframes()*T//wav.getframerate()
  imported=next(c for c in clips if c['item']==item)
  assert imported['duration']==expected,'generated narration was rounded to a video frame'
 export=root/'ai-edit.mp4';job=ex('file.exportMedia',path=str(export),format='h264',width=160,height=90,fps=24,audio=True)['job']
 for _ in range(1200):
  jobs=ex('jobs.list');state=next(j for j in jobs if j['id']==job)
  if state.get('result') is not None:assert not state['result'].get('error'),state;break
  time.sleep(.05)
 else:raise AssertionError('export timed out')
 probe=json.loads(subprocess.check_output([a.ffprobe,'-v','error','-show_streams','-show_format','-of','json',str(export)]));(root/'ffprobe.json').write_text(json.dumps(probe,indent=2))
 video=next(s for s in probe['streams'] if s['codec_type']=='video');audio=next(s for s in probe['streams'] if s['codec_type']=='audio')
 assert (video['width'],video['height'])==(160,90);assert abs(float(video['duration'])-saved['duration']/T)<1/24
 assert abs(float(audio['duration'])-float(video['duration']))<.1
 pixels=subprocess.check_output([a.ffmpeg,'-v','error','-i',str(export),'-f','rawvideo','-pix_fmt','rgb24','-']);size=160*90*3;assert len(pixels)%size==0
 for at in range(0,len(pixels),size):
  r,g,b=pixels[at+(45*160+80)*3:at+(45*160+80)*3+3];assert r>220 and g<25 and b<25
 output=decode_audio(export);reference=decode_audio(es_file);gain=10**(-3/20);n=min(len(reference),len(output));assert n>48000
 expected=[v*gain for v in reference[:n]]
 # AAC lossy reconstruction can add a small shift: measure maximum correlation within 64 samples.
 correlations=[correlation(expected[2048:-2048],output[2048+shift:n-2048+shift]) for shift in range(-64,65,8)]
 assert max(correlations)>.8,correlations
 en_reference=decode_audio(en_file);offset=round((en_start+.5)*48000);n=min(len(en_reference)-48000,len(output)-offset)
 assert n>48000;en_corr=max(correlation(en_reference[2048:n-2048],output[offset+2048+shift:offset+n-2048+shift]) for shift in range(-64,65,8));assert en_corr>.8,en_corr
 # Local Autopilot executes tasks, then cancellation cannot leave future edits armed.
 tool('editor_autopilot',instructions=['remove silence'],apply=True,local=True);auto=wait_ai();assert auto['status']['completed']==1,auto
 tool('ai_cancel');assert not tool('ai_job_status')['busy']
 result={'voices':sorted(v['id'] for v in voices),'videoFrames':len(pixels)//size,'duration':video['duration'],'spanishCorrelation':max(correlations),'englishCorrelation':en_corr,'playback':playing['playback'],'autopilot':auto['status'],'project':str(project),'export':str(export),'comfyNeuralCheckpointTested':False}
 (root/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2),flush=True)
finally:
 stream.close();sock.close();proc.stdin.close()
 try:proc.wait(timeout=10)
 except subprocess.TimeoutExpired:proc.terminate();proc.wait(timeout=10)
 log.close()
