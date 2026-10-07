"""Real desktop E2E over the control socket; only Python stdlib and external FFmpeg oracles.

Start FilmCraft with --empty --no-recover --control 9876, then run this script. All media is
original and generated under --out; no FFmpeg code or binaries are linked or shipped.
"""
import argparse
import shutil
import array
import json
import math
import os
import pathlib
import socket
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port', type=int, default=9876)
parser.add_argument('--out', type=pathlib.Path, default=pathlib.Path('target/native-e2e'))
parser.add_argument('--ffmpeg', default=os.environ.get('FILMCRAFT_FFMPEG') or shutil.which('ffmpeg'))
parser.add_argument('--ffprobe', default=os.environ.get('FILMCRAFT_FFPROBE') or shutil.which('ffprobe'))
args = parser.parse_args()
if not args.ffmpeg or not args.ffprobe:
    parser.error('FFmpeg and FFprobe are required external test oracles')
ffmpeg, ffprobe = args.ffmpeg, args.ffprobe
ROOT = args.out.resolve()
(ROOT/'media').mkdir(parents=True, exist_ok=True)
for color, frequency in [('red',440), ('blue',880)]:
    subprocess.run([ffmpeg,'-v','error','-y','-f','lavfi','-i',f'color=c={color}:s=160x90:r=24:d=4',
                    '-f','lavfi','-i',f'sine=frequency={frequency}:sample_rate=48000:duration=4',
                    '-c:v','libx264','-pix_fmt','yuv420p','-c:a','aac','-threads','1',
                    str(ROOT/'media'/f'{color}.mp4')],check=True)
subprocess.run([ffmpeg,'-v','error','-y','-f','lavfi','-i','color=c=green:s=40x30',
                '-frames:v','1','-threads','1',str(ROOT/'media'/'still.png')],check=True)
subprocess.run([ffmpeg,'-v','error','-y','-f','lavfi','-i','sine=frequency=220:sample_rate=48000:duration=3',
                str(ROOT/'media'/'tone.wav')],check=True)
TICKS = 254016000000
sock = socket.create_connection(('127.0.0.1', args.port), timeout=15)
sock.settimeout(60)
stream = sock.makefile('rwb')
trace = []

def call(method, params=None, expected_ok=True):
    request = {'id': len(trace), 'method': method, 'params': params or {}}
    stream.write((json.dumps(request) + '\n').encode())
    stream.flush()
    response = json.loads(stream.readline())
    trace.append({'request': request, 'response': response})
    (ROOT / 'native-trace.json').write_text(json.dumps(trace, indent=2))
    assert response['ok'] == expected_ok, response
    return response.get('result') if expected_ok else response.get('error')

def ex(command, **params):
    return call('engine.execute', {'command': command, 'params': params})

def sequence():
    result = ex('sequence.inspect')
    result.pop('selection', None)
    result.pop('playhead', None)
    return result

def screenshot(name, seconds):
    ex('playhead.set', seconds=seconds)
    time.sleep(.3)
    return call('ui.screenshot', {'path': str(ROOT / name), 'panel': 'Program'})

def wait_job(job):
    until = time.monotonic() + 120
    while time.monotonic() < until:
        jobs = ex('jobs.list')
        item = next(j for j in jobs if j['id'] == job)
        if item['finished']:
            assert not item.get('result', {}).get('error'), item
            return item
        time.sleep(.05)
    raise AssertionError('job timeout')

ex('file.newProject', name='Verified editing')
ex('file.newSequence', name='Actual edit', width=160, height=90, fps=24, video=2, audio=2)
media = ex('file.import', paths=[str(ROOT / 'media' / name) for name in ['red.mp4', 'blue.mp4', 'still.png', 'tone.wav']])
assert not media.get('errors'), media
assert len(media['items']) == 4, media
red, blue, still, sound = media['items']
a = ex('timeline.place', item=red, track='V1', audioTrack='A1', seconds=0, duration=2*TICKS)['clips']
b = ex('timeline.place', item=blue, track='V1', audioTrack='A1', seconds=2, sourceIn=TICKS, duration=2*TICKS)['clips']
ex('timeline.trim', clip=a[0], edge='out', mode='regular', deltaFrames=-12)
ex('timeline.move', moves=[{'clip': b[0], 'track': 'V1', 'time': int(1.5*TICKS)}])
ex('timeline.razor', clip=b[0], time=int(2.5*TICKS))
edited = sequence()
ex('timeline.select', clips=[a[0]])
ex('edit.clear')
ex('edit.undo')
assert sequence() == edited, 'clear/undo must restore the entire sequence'
ex('edit.redo')
ex('edit.undo')
assert sequence() == edited
for second in [0, 3.2, .1, 1.5, 2.75, 0, 3.49, .5]:
    ex('playhead.set', seconds=second)
    assert abs(call('ui.inspect')['playhead']/TICKS - second) <= 1/24
screenshot('native-red.png', .5)
screenshot('native-blue.png', 2.75)
ex('playhead.set', seconds=0)
call('ui.playback', {'action': 'play'})
deadline = time.monotonic() + 10
while time.monotonic() < deadline:
    ready = call('ui.inspect')
    if not ready['playback']['preroll'] and ready['playhead'] > 0:
        break
    time.sleep(.05)
else:
    raise AssertionError('playback did not finish preroll and present its first frame')
assert ready['playback']['playing'], ready['playback']
time.sleep(.4)
play = call('ui.inspect')
advance = (play['playhead'] - ready['playhead']) / TICKS
assert .15 < advance < 2, play['playback']
assert play['playback']['shown'] > ready['playback']['shown'], 'playback clock advanced without displaying frames'
call('ui.playback', {'action': 'stop'})
ex('playhead.set', seconds=2.75)
project = ROOT / 'edit.fcproj'
ex('file.saveAs', path=str(project))
saved = sequence()
ex('file.closeProject')
ex('file.open', path=str(project))
assert sequence() == saved, 'save/reopen must preserve the edit'
restored_playhead = call('ui.inspect')['playhead']
assert restored_playhead == int(2.75*TICKS), 'save/reopen playhead'

output = ROOT / 'actual-edit.mp4'
job = ex('file.exportMedia', path=str(output), format='h264', width=160, height=90, fps=24, audio=True)['job']
result = wait_job(job)
assert output.stat().st_size > 1000
probe = json.loads(subprocess.check_output([ffprobe, '-v', 'error', '-show_streams', '-show_format', '-of', 'json', str(output)]))
video = next(s for s in probe['streams'] if s['codec_type'] == 'video')
audio = next(s for s in probe['streams'] if s['codec_type'] == 'audio')
assert (video['width'], video['height'], video['codec_name']) == (160, 90, 'h264')
assert audio['codec_name'] == 'aac'
assert audio['sample_rate'] == '48000' and audio['duration_ts'] == 168000, 'AAC presentation must be exactly 3.5 seconds'
assert abs(float(probe['format']['duration']) - 3.5) < .1
pixels = subprocess.check_output([ffmpeg, '-v', 'error', '-i', str(output), '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'])
size = 160*90*3
assert len(pixels)//size == 84
for frame in range(84):
    pixel = pixels[frame*size+size//2:frame*size+size//2+3]
    r,g,b = pixel
    assert (r > 230 and b < 20) if frame < 36 else (b > 230 and r < 20), (frame, list(pixel))
pcm = subprocess.check_output([ffmpeg, '-v', 'error', '-i', str(output), '-vn', '-ac', '1', '-ar', '48000', '-f', 'f32le', '-'])
samples = array.array('f');samples.frombytes(pcm)
tones = []
for second, expected in [(.3,440), (1.8,880), (2.8,880)]:
    chunk = samples[int(second*48000):int((second+.1)*48000)]
    def amplitude(hz):
        return 2*abs(sum(value*complex(math.cos(2*math.pi*hz*i/48000), -math.sin(2*math.pi*hz*i/48000)) for i,value in enumerate(chunk)))/len(chunk)
    wanted, other = amplitude(expected), amplitude(880 if expected==440 else 440)
    assert wanted > .04 and other < wanted*.1, (second, wanted, other)
    tones.append({'second':second, 'frequency':expected, 'amplitude':wanted, 'other':other})
# Missing-file recovery uses actual disk moves and the production relinker.
blue_path = ROOT/'media'/'blue.mp4'
moved = ROOT/'media'/'blue-moved.mp4'
blue_path.rename(moved)
try:
    opened = ex('file.open', path=str(project))
    assert opened['missingMedia'] == 1, opened
    ex('media.relink', item=blue, path=str(moved), match={'fileName':False}, relinkOthers=False)
    screenshot('native-relinked.png', 2.75)
finally:
    moved.rename(blue_path)
ex('file.open', path=str(project))
invalid = ROOT/'media'/'broken.mp4'
invalid.write_bytes(b'not a movie')
mixed_import = ex('file.import', paths=[str(invalid),str(ROOT/'media'/'tone.wav')])
assert len(mixed_import['errors']) == 1 and len(mixed_import['items']) == 1, mixed_import

# Cancel after actual encoding begins; a previous destination must remain byte-exact.
ex('file.newProject')
ex('file.newSequence', width=320, height=180, fps=24, video=1, audio=1)
generator = ex('file.newBarsAndTone', seconds=30)['item']
ex('timeline.place', item=generator, track='V1', audioTrack='A1', seconds=0, duration=30*TICKS)
cancel_path = ROOT/'cancel-overwrite.mp4'
previous = b'previous export sentinel'
cancel_path.write_bytes(previous)
cancel_job = ex('file.exportMedia', path=str(cancel_path), format='h264')['job']
for _ in range(600):
    cancel = next(j for j in ex('jobs.list') if j['id'] == cancel_job)
    if cancel['done'] > 0:
        break
    time.sleep(.05)
else:
    raise AssertionError('encoding never started')
assert not cancel['finished'], 'fixture should still be exporting'
ex('jobs.cancel', job=cancel_job)
for _ in range(600):
    cancel = next(j for j in ex('jobs.list') if j['id'] == cancel_job)
    if cancel['finished']:
        break
    time.sleep(.05)
else:
    raise AssertionError('cancelled job never finished')
assert cancel['result']['error'] == 'cancelled', cancel
assert cancel_path.read_bytes() == previous, 'cancellation destroyed the previous destination'
assert not list(ROOT.glob('.filmcraft-export-*.part')), 'incomplete staging files were not cleaned'
ex('file.open', path=str(project))
before_bad_open = sequence()
corrupt = json.loads(project.read_text())
bad_seq = next(i['kind']['Sequence'] for i in corrupt['project']['items'].values() if 'Sequence' in i['kind'])
next(t for t in bad_seq['video_tracks'] if t['items'])['items'][0]['start'] = 2**63 - 1
corrupt_path = ROOT / 'corrupt.fcproj'
corrupt_path.write_text(json.dumps(corrupt))
error = call('engine.execute', {'command':'file.open', 'params':{'path':str(corrupt_path)}}, expected_ok=False)
assert 'time bounds' in str(error), error
assert sequence() == before_bad_open, 'opening a corrupt project changed the current edit'
report = {'edgeCases': {'missingMedia':True, 'relinked':True, 'invalidMedia':True, 'invalidProjectPreservesSession':True, 'cancelPreservesDestination':True}, 'playback': {'seconds':play['playhead']/TICKS, 'state':play['playback']}, 'export':result, 'probe':probe, 'videoFrames':84, 'cutsAtFrame':36, 'audioTones':tones, 'savedProject':str(project)}
(ROOT/'native-e2e-report.json').write_text(json.dumps(report, indent=2))
print(json.dumps(report, indent=2))
