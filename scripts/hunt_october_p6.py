# /// script
# dependencies = ["numpy", "scipy", "zstandard"]
# ///
"""Diagnostic P6-first search independent of the P1/P2 detector.
Retains exact known-address matches, never repairs arbitrary bits.
"""
import argparse,json
from pathlib import Path
import numpy as np
from scipy.ndimage import uniform_filter1d
import zstandard as zstd

p=argparse.ArgumentParser();p.add_argument('--input',default='/tmp/october-filtered.raw.zst');p.add_argument('--output',default='/tmp/oct-p6-matches.jsonl');p.add_argument('--known-aircraft',default='opensky_20261005_112322/known_aircraft.csv');p.add_argument('--valid-uf0-only',action='store_true');args=p.parse_args()
fs=16_000_000;spb=4
known=[int(line.split(',')[0],16) for line in Path(args.known_aircraft).read_text().splitlines()[1:]]
def overlay(addr):
 product=0
 for i in range(24):
  if addr&(1<<i):product^=0x1fff409<<i
 return (product>>24)&0xffffff
known_overlay={overlay(addr):addr for addr in known}
lut=[]
for b in range(256):
 crc=b<<16
 for _ in range(8):crc=((crc<<1)^ (0xfff409 if crc&0x800000 else 0))&0xffffff
 lut.append(crc)
def crc_prefix(data):
 crc=0
 for byte in data:crc=((crc<<8)|int(byte))^lut[(crc>>16)&255];crc&=0xffffff
 # Required x^24 augmentation for Mode S parity (Annex 10 IV).
 for _ in range(3):crc=((crc<<8)^lut[(crc>>16)&255])&0xffffff
 return crc
base=0;candidate_count=0;matches=[];seen=set()
with open(args.input,'rb') as f,zstd.ZstdDecompressor().stream_reader(f) as r:
 while True:
  raw=r.read(fs*8)
  if not raw:break
  a=np.frombuffer(raw,dtype='<f4');x=a[::2]+1j*a[1::2]
  mag=np.abs(x);smooth=uniform_filter1d(mag,size=8)
  threshold=max(.025,float(np.median(smooth))*3)
  rising=np.flatnonzero((smooth[1:]>threshold)&(smooth[:-1]<=threshold))+1
  # P6 should sustain substantial energy over at least the short data extent.
  energy=uniform_filter1d(mag,size=180)
  rising=rising[(rising>40)&(rising<len(x)-600)]
  rising=rising[energy[rising+120]>threshold]
  # Avoid many noise threshold crossings within one burst.
  if len(rising):rising=rising[np.r_[True,np.diff(rising)>12]]
  candidate_count+=len(rising)
  for onset in rising:
   # P6 leading edge or P1 leading edge; include p6 at +3.5us from P1.
   for anchor in [int(onset),int(onset)+56]:
    for off in range(-16,17):
     data=anchor+off+28
     z=x[data-4:data+448]
     if len(z)!=452:continue
     corr=(z[4:]*z[:-4].conj()).reshape(112,4).sum(1)
     for bits in ([56] if args.valid_uf0_only else [56,112]):
      c=corr[:bits]
      phase=.5*np.angle(np.sum(c*c))
      for rot in [0,phase]:
       values=(c*np.exp(-1j*rot)).real<0
       for inv in [False,True]:
        frame=np.packbits(values^inv)
        uf=int(frame[0])>>3
        if args.valid_uf0_only:
         if bits != 56 or uf != 0:continue
         payload=int.from_bytes(frame[:4].tobytes(),'big')
         if payload & (~0x0087fc00 & 0xffffffff):continue
        if uf not in ([0,4,5] if bits==56 else [16,20,21,24,25,26,27,28,29,30,31]):continue
        parity=int.from_bytes(frame[-3:].tobytes(),'big')
        addr=known_overlay.get(crc_prefix(frame[:-3])^parity)
        if addr is None:continue
        key=(round((base+anchor)/800),addr,frame.tobytes())
        if key in seen:continue
        seen.add(key)
        row={'seconds':(base+anchor+off)/fs,'icao24':f'{addr:06x}','uf':24 if uf>=24 else uf,'raw':frame.tobytes().hex(),'offset':off,'phase_rotation_rad':float(rot),'inverted':inv,'mean_confidence':float(np.mean(abs((c*np.exp(-1j*rot)).real)/(abs(c)+1e-12)))}
        matches.append(row)
  base+=len(x)
  print(f'{base/fs:.1f}s candidates={candidate_count} matches={len(matches)}',flush=True)
Path(args.output).write_text(''.join(json.dumps(row)+'\n' for row in matches))
print(args.output)
