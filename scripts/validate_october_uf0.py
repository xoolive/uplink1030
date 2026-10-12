# /// script
# dependencies = ["numpy", "scipy", "zstandard", "matplotlib"]
# ///
"""Inspect IQ preambles for independently recovered October UF0 candidates."""
import json
import argparse
from pathlib import Path
import numpy as np
import zstandard as zstd
from scipy.signal import butter,sosfilt
import matplotlib.pyplot as plt
parser=argparse.ArgumentParser()
parser.add_argument('--output-dir',default='opensky_20261005_112322')
parser.add_argument('--input',default=str(Path.home()/'Library/Mobile Documents/com~apple~CloudDocs/gqrx_20261005_112322_1030000000_16000000_fc.raw.zst'))
parser.add_argument('--frequency-offset-hz',type=float,default=0)
parser.add_argument('--min-seconds',type=float,default=240)
args=parser.parse_args()
out=Path(args.output_dir)
rows=[json.loads(l) for l in (out/'p6_valid_uf0.jsonl').open()]
p=Path(args.input).expanduser()
fs=16_000_000;sos=butter(6,3_500_000,fs=fs,output='sos');zi=np.zeros((3,2),complex)
windows={};base=0
with p.open('rb') as f,zstd.ZstdDecompressor().stream_reader(f) as r:
 while True:
  raw=r.read(fs*8)
  if not raw:break
  a=np.frombuffer(raw,dtype='<f4');x=a[::2]+1j*a[1::2]
  if base/fs>=args.min_seconds:
   x=x-np.mean(x,dtype=np.complex128)
   if args.frequency_offset_hz:
    x=x*np.exp(-2j*np.pi*args.frequency_offset_hz*((base+np.arange(len(x)))%fs)/fs)
   y,zi=sosfilt(sos,x,zi=zi)
   for i,row in enumerate(rows):
    s=round(row['seconds']*fs)-base
    if 120<s<len(x)-500:
     z=y[s-112:s+400];windows[i]=z
     # Adjacent one-symbol windows around the nominal sync reversal at 1.25us.
     sync=112+20
     c=np.sum(z[sync:sync+4]*np.conj(z[sync-4:sync]))
     row['sync_normalized_real']=float(c.real/(np.linalg.norm(z[sync:sync+4])*np.linalg.norm(z[sync-4:sync])+1e-20))
     row['p1_magnitude']=float(np.mean(abs(z[112-56:112-43])))
     row['p2_magnitude']=float(np.mean(abs(z[112-24:112-11])))
  base+=len(x)
fig,axs=plt.subplots(4,1,figsize=(11,9),constrained_layout=True)
plot_indices=[0,len(rows)//3,2*len(rows)//3,len(rows)-1]
for ax,i in zip(axs,plot_indices):
 z=windows[i];t=(np.arange(len(z))-112)/fs*1e6
 ax.plot(t,abs(z),color='tab:blue',lw=1)
 for pos,label in [(-3.5,'P1'),(-1.5,'P2'),(0,'P6'),(1.25,'sync')]:
  ax.axvline(pos,color='gray',ls='--',alpha=.5)
 ax.set(title=f"{rows[i]['icao24']} at +{rows[i]['seconds']:.6f} s",ylabel='Envelope',xlim=(-5,22))
axs[-1].set_xlabel('Time relative to recovered P6 (µs)')
fig.savefig(out/'recovered_uf0_waveforms.png',dpi=180)
(out/'p6_valid_uf0_validated.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in rows))
valid=[row['sync_normalized_real'] for row in rows if 'sync_normalized_real' in row]
print('P6 sync validation:',len(valid),'frames;',sum(x<0 for x in valid),'with negative sync correlation')
print('Sync correlation percentiles:',np.percentile(valid,[10,50,90]))
