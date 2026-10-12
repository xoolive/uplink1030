# /// script
# dependencies = ["numpy", "matplotlib"]
# ///
"""Audit timing in both captures, including cross-register held-out checks.
Does not overwrite the original alignment outputs or force causal RF matches.
"""
import collections,csv,datetime,json
from pathlib import Path
import numpy as np
import matplotlib.pyplot as plt

ROOT=Path('timing_consistency_20261006');ROOT.mkdir(exist_ok=True)
CAPTURES=[('105707',1791284227.,163.9955469375),('110053',1791284453.,294.121728)]
reference=collections.defaultdict(list)
for line in (Path.home()/'gqrx_20261006_110053_1032000000_16000000_fc.jsonl').open():
 if not line.strip():continue
 d=json.loads(line)
 for bds in ('0x40','0x50','0x60'):
  if 'bds'+bds[2:] in d:reference[(d['icao24'],int(d['df']),bds)].append(d['timestamp'])

def stats(values):
 v=np.asarray(values,dtype=float)
 if not len(v):return {'n':0}
 return {'n':len(v),'median_ms':float(np.median(v)), 'mean_ms':float(np.mean(v)), 'std_ms':float(np.std(v)), 'p05_ms':float(np.percentile(v,5)), 'p95_ms':float(np.percentile(v,95)), 'min_ms':float(v.min()), 'max_ms':float(v.max())}

def mode(lags):
 if not len(lags):return None,0
 v=np.asarray(lags)
 grid=np.arange(-1,3.0025,.005)
 scores=np.array([np.maximum(0,1-abs(v-c)/.015).sum() for c in grid])
 peak=grid[scores.argmax()];keep=abs(v-peak)<.015
 return float(np.median(v[keep])),int(keep.sum())

fig,axs=plt.subplots(3,2,figsize=(14,12),constrained_layout=True)
summary={};binned=[];heldout_rows=[]
for col,(tag,start,duration) in enumerate(CAPTURES):
 folder=Path('opensky_20261006_'+tag)
 rows=list(csv.DictReader((folder/'uplink_downlink_alignment.csv').open()))
 ground=[r for r in rows if r['UF'] in ('4','5')]
 matched=[r for r in rows if r['DF']]
 ground_matched=[r for r in ground if r['DF']]
 models=json.loads((folder/'reply_clock_model.json').read_text())
 pairs=[]
 for i,r in enumerate(ground):
  bds=r['BDS_query']
  if bds not in ('0x40','0x50','0x60'):continue
  t=float(r['iq_seconds'])
  for reply in reference[(r['icao24'],int(r['expected_DF']),bds)]:
   lag=reply-start-t
   if -1<=lag<=3:pairs.append({'i':i,'t':t,'lag':lag,'bds':bds})
 # Train on UF4 BDS5,0 only. Test UF5 BDS4,0 and BDS6,0 only.
 train=[p for p in pairs if p['bds']=='0x50' and ground[p['i']]['UF']=='4']
 heldout_models=[]
 for center in np.arange(5,duration,10):
  lag,support=mode([p['lag'] for p in train if abs(p['t']-center)<15])
  heldout_models.append({'center_s':float(center),'lag_s':lag,'support':support,'usable':support>=3})
 # ±100ms diagnostic window is deliberately wider than the original ±15ms.
 # Retain only unique held-out register-compatible pairs in that broad window.
 heldout=[];absent=ambiguous=unsupported=0
 fixed_lag,fixed_support=mode([p['lag'] for p in train if p['t']<30])
 fixed_residuals=[]
 for i,r in enumerate(ground):
  if r['UF']!='5' or r['BDS_query'] not in ('0x40','0x60'):continue
  t=float(r['iq_seconds']);m=min(heldout_models,key=lambda m:abs(m['center_s']-t))
  candidates=[p for p in pairs if p['i']==i]
  if fixed_lag is not None:
   near=[p for p in candidates if abs(p['lag']-fixed_lag)<=.1]
   if len(near)==1:fixed_residuals.append(1000*(near[0]['lag']-fixed_lag))
  if not m['usable']:unsupported+=1;continue
  near=[p for p in candidates if abs(p['lag']-m['lag_s'])<=.1]
  if not near:absent+=1;continue
  if len(near)>1:ambiguous+=1;continue
  p=near[0];residual=1000*(p['lag']-m['lag_s']);heldout.append((t,residual))
  heldout_rows.append({'capture':tag,'iq_seconds':t,'icao24':r['icao24'],'BDS_query':r['BDS_query'],'raw_lag_s':p['lag'],'clock_offset_s':m['lag_s'],'training_support':m['support'],'heldout_residual_ms':residual})
 bins=[]
 for a in np.arange(0,duration,30):
  b=min(a+30,duration)
  ss=[r for r in ground_matched if a<=float(r['iq_seconds'])<b]
  vv=[float(r['corrected_time_residual_ms']) for r in ss]
  item={'capture':tag,'start_s':float(a),'stop_s':float(b),**stats(vv)}
  if ss:item.update({'offset_min_s':min(float(r['clock_offset_s']) for r in ss),'offset_max_s':max(float(r['clock_offset_s']) for r in ss)})
  bins.append(item);binned.append(item)
 result={'assigned_all':stats([float(r['corrected_time_residual_ms']) for r in matched]),'assigned_ground':stats([float(r['corrected_time_residual_ms']) for r in ground_matched]), 'assigned_uf0':stats([float(r['corrected_time_residual_ms']) for r in matched if r['UF']=='0']), 'local_clock_min_s':min(m['lag_s'] for m in models if m['usable']), 'local_clock_max_s':max(m['lag_s'] for m in models if m['usable']), 'usable_clock_bins':sum(m['usable'] for m in models),'total_clock_bins':len(models), 'ground_time_bins':bins, 'cross_register_heldout':stats([v for _,v in heldout]), 'cross_register_heldout_within15ms':sum(abs(v)<=15 for _,v in heldout), 'cross_register_test_no_reply':absent,'cross_register_test_ambiguous':ambiguous,'cross_register_test_no_clock':unsupported,'fixed_first30s_training_lag_s':fixed_lag,'fixed_first30s_training_support':fixed_support,'fixed_first30s_heldout':stats(fixed_residuals),'cross_register_models':heldout_models}
 # Independent Rust and P6 sample-time anchors for shared exact UF0 frames.
 p6_by=collections.defaultdict(list)
 for line in (folder/'p6_valid_uf0.jsonl').open():
  d=json.loads(line);p6_by[(d['icao24'],d['raw'])].append(d['seconds'])
 shared=[]
 for line in (folder/'corrected_crc_uplinks.jsonl').open():
  d=json.loads(line);m=d['message']
  if m['type']!='uf0':continue
  t=d['detection']['p6_sample']/16e6
  near=[v for v in p6_by[(m['icao24'],m['raw'])] if abs(v-t)<.0001]
  if near:shared.append(1e6*(min(near,key=lambda v:abs(v-t))-t))
 result['shared_rust_p6_time_check']={'n':len(shared),'min_us':float(min(shared)),'median_us':float(np.median(shared)),'max_us':float(max(shared))}
 summary[tag]=result
 ax=axs[0,col]
 ax.scatter([p['t'] for p in pairs],[p['lag'] for p in pairs],s=9,alpha=.15,color='tab:blue')
 # Plot raw assigned ground lags using stored corrections + residuals.
 ax.scatter([float(r['iq_seconds']) for r in ground_matched],[float(r['clock_offset_s'])+float(r['corrected_time_residual_ms'])/1000 for r in ground_matched],s=20,color='tab:orange',label='assigned ground pairs')
 for m in models:
  if m['usable']:ax.plot([m['center_s']-5,m['center_s']+5],[m['lag_s'],m['lag_s']],color='tab:red',lw=2)
 ax.set(title=f'{tag}: raw software-clock lag',ylabel='Reference − nominal IQ time (s)',ylim=(-1,3));ax.legend(loc='lower left')
 ax=axs[1,col]
 for uf,color in [('0','gray'),('4','tab:blue'),('5','tab:orange')]:
  rr=[r for r in matched if r['UF']==uf]
  ax.scatter([float(r['iq_seconds']) for r in rr],[float(r['corrected_time_residual_ms']) for r in rr],s=16,alpha=.65,label='UF'+uf,color=color)
 ax.axhline(0,color='k',lw=.6);ax.axhline(15,color='r',ls='--',lw=.6);ax.axhline(-15,color='r',ls='--',lw=.6)
 ax.set(title='Original locally corrected, selected matches',ylabel='Residual (ms)',ylim=(-20,20));ax.legend()
 ax=axs[2,col]
 ax.scatter([t for t,v in heldout],[v for t,v in heldout],s=22,color='tab:green',label='UF5 held out; trained on UF4/BDS5,0')
 ax.axhline(0,color='k',lw=.6);ax.axhline(15,color='r',ls='--',lw=.6);ax.axhline(-15,color='r',ls='--',lw=.6)
 ax.set(title='Cross-register held-out check (±100ms search)',ylabel='Held-out residual (ms)',ylim=(-105,105),xlabel='Time into capture (s)');ax.legend(fontsize=8)
 for ax in axs[:,col]:ax.set_xlim(0,duration);ax.grid(alpha=.2)
(ROOT/'timing_summary.json').write_text(json.dumps(summary,indent=2)+'\n')
with (ROOT/'ground_residuals_30s.csv').open('w') as f:
 fields=['capture','start_s','stop_s','n','median_ms','mean_ms','std_ms','p05_ms','p95_ms','min_ms','max_ms','offset_min_s','offset_max_s'];w=csv.DictWriter(f,fieldnames=fields);w.writeheader();w.writerows(binned)
with (ROOT/'heldout_uf5_residuals.csv').open('w') as f:
 fields=['capture','iq_seconds','icao24','BDS_query','raw_lag_s','clock_offset_s','training_support','heldout_residual_ms'];w=csv.DictWriter(f,fieldnames=fields);w.writeheader();w.writerows(heldout_rows)
fig.savefig(ROOT/'timing_consistency.png',dpi=180)
print(json.dumps({k:{a:b for a,b in v.items() if a not in ('ground_time_bins','cross_register_models')} for k,v in summary.items()},indent=2))
