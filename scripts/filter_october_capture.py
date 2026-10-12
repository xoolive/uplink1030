# /// script
# dependencies = ["numpy", "scipy", "zstandard"]
# ///
"""Channelize October CF32 capture; original is never modified."""
import argparse
from pathlib import Path
import numpy as np
from scipy.signal import butter, sosfilt
import zstandard as zstd

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=30)
parser.add_argument('--output', default='/tmp/october-filtered.raw.zst')
parser.add_argument('--input', default=str(Path.home()/'Library/Mobile Documents/com~apple~CloudDocs/gqrx_20261005_112322_1030000000_16000000_fc.raw.zst'))
parser.add_argument('--frequency-offset-hz', type=float, default=0, help='Signal RF frequency minus recording center frequency')
a=parser.parse_args()
p=Path(a.input).expanduser()
fs=16_000_000
sos=butter(6, 3_500_000, fs=fs, output='sos')
zi=np.zeros((len(sos),2),dtype=np.complex128)
n=0
with p.open('rb') as f, zstd.ZstdDecompressor().stream_reader(f) as r, open(a.output,'wb') as o, zstd.ZstdCompressor(level=1).stream_writer(o) as w:
 while n < int(a.seconds*fs):
  raw=r.read(min(8*fs,8*(int(a.seconds*fs)-n)))
  if not raw:break
  v=np.frombuffer(raw,dtype='<f4'); x=v[::2]+1j*v[1::2]
  dc=np.mean(x,dtype=np.complex128)
  x=x-dc
  if a.frequency_offset_hz:
   phase=-2*np.pi*a.frequency_offset_hz*((n+np.arange(len(x)))%fs)/fs
   x=x*np.exp(1j*phase)
  y,zi=sosfilt(sos,x,zi=zi)
  vout=np.empty(2*len(y),dtype='<f4');vout[::2]=y.real;vout[1::2]=y.imag
  w.write(vout.tobytes());n+=len(y)
  if n % (10*fs)==0:print(f'{n/fs:.1f}s DC {dc} peak {abs(y).max():.3f}',flush=True)
print(a.output,n/fs)
