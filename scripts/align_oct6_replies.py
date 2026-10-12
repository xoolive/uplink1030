# /// script
# dependencies = ["numpy", "matplotlib"]
# ///
"""Conservative, one-to-one candidate reply alignment using local clock calibration.
Reference timestamps are system-clock observations, not RF arrival timestamps.
"""
import argparse,csv,json,collections,datetime
from pathlib import Path
import numpy as np
import matplotlib.pyplot as plt

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output-dir',default='opensky_20261006_110053')
parser.add_argument('--reference',default=str(Path.home()/'gqrx_20261006_110053_1032000000_16000000_fc.jsonl'))
parser.add_argument('--start-utc',default='2026-10-06T11:00:53+00:00')
parser.add_argument('--duration',type=float,default=294.121728)
parser.add_argument('--clock-lag-min',type=float,default=.5)
parser.add_argument('--clock-lag-max',type=float,default=2.5)
parser.add_argument('--calibration-iis',type=int,default=3,help='IIS used for ground-query clock calibration; -1 allows all')
args=parser.parse_args()
start_dt=datetime.datetime.fromisoformat(args.start_utc)
if start_dt.tzinfo is None:parser.error('--start-utc must include a timezone')
if args.duration<=0 or args.clock_lag_max<=args.clock_lag_min:parser.error('invalid duration or clock-lag interval')
OUT=Path(args.output_dir)
START=start_dt.timestamp()
reference=[]
with Path(args.reference).expanduser().open() as f:
 for line in f:
  if not line.strip():continue
  d=json.loads(line)
  if START-20 <= d['timestamp'] <= START+args.duration+20:
   d['_df']=int(d['df']);d['_time']=d['timestamp']-START
   reference.append(d)
reference.sort(key=lambda d:d['_time'])
refs={d['icao24']:d for d in csv.DictReader((OUT/'reference_aircraft_summary.csv').open())}
ups=[]
for line in (OUT/'corrected_crc_uplinks.jsonl').open():
 d=json.loads(line);m=d['message'];uf=int(m['type'][2:]);rr=m.get('rr')
 expected=None
 if uf==0:expected=16 if m.get('rl') else 0
 elif uf==16:expected=16
 elif uf in (4,20):expected=20 if rr is not None and rr>=16 else 4
 elif uf in (5,21):expected=21 if rr is not None and rr>=16 else 5
 ups.append({'time':d['detection']['p6_sample']/16e6,'icao24':m.get('icao24',''),'uf':uf,'expected_df':expected,'bds':m.get('bds') or '', 'iis':m.get('sd',{}).get('iis',''), 'raw':m['raw'],'source':'rust','provisional':uf in (16,20,21,24)})
# Add independently recovered UF0; deduplicate identical target/frame within100us.
for line in (OUT/'p6_valid_uf0.jsonl').open():
 d=json.loads(line)
 ups.append({'time':d['seconds'],'icao24':d['icao24'],'uf':0,'expected_df':16 if int(d['raw'][:8],16)&0x00800000 else 0,'bds':'','iis':'','raw':d['raw'],'source':'p6','provisional':False})
merged=[];last={}
for u in sorted(ups,key=lambda d:d['time']):
 key=u['icao24'],u['raw'];old=last.get(key)
 if old and u['time']-old['time']<.0001:
  if u['source'] not in old['source']:old['source']+='+'+u['source']
 else:merged.append(u);last[key]=u
ups=merged

# Calibration uses only same-address, correct-DF, correct-register ground pairs.
index=collections.defaultdict(list)
for j,d in enumerate(reference):index[(d['icao24'],d['_df'])].append((d['_time'],j))
calibration=[]
for u in ups:
 if u['uf'] not in (4,5) or (args.calibration_iis>=0 and u['iis']!=args.calibration_iis) or u['bds'] not in ('0x20','0x40','0x50','0x60'):continue
 field='bds'+u['bds'][2:]
 for time,j in index[(u['icao24'],u['expected_df'])]:
  lag=time-u['time']
  if args.clock_lag_min<=lag<=args.clock_lag_max and field in reference[j]:calibration.append((u['time'],lag))
calibration=np.array(calibration,dtype=float).reshape(-1,2)
grid=np.arange(args.clock_lag_min,args.clock_lag_max+.0025,.005);models=[]
for center in np.arange(5,max(5.001,args.duration),10):
 selected=calibration[abs(calibration[:,0]-center)<15]
 v=selected[:,1]
 scores=np.array([np.maximum(0,1-abs(v-lag)/.015).sum() for lag in grid])
 peak=grid[scores.argmax()];keep=abs(v-peak)<.015
 support=int(keep.sum())
 models.append({'center_s':float(center),'lag_s':float(np.median(v[keep])) if support else None,'support':support,'usable':support>=6})
(OUT/'reply_clock_model.json').write_text(json.dumps(models,indent=2)+'\n')

# One-to-one matching, no reuse of reference frames. Calibrations with few
# supporting ground queries are not used. Do not force a match for noisy UFs.
potential=[];statuses={};matching_tolerance=.015
for i,u in enumerate(ups):
 model=min(models,key=lambda m:abs(m['center_s']-u['time']))
 u['_model']=model
 if u['provisional']:statuses[i]='unconfirmed_uplink';continue
 if u['expected_df'] is None:statuses[i]='no_reply_rule';continue
 if not model['usable']:statuses[i]='clock_not_calibrated';continue
 target=u['time']+model['lag_s']
 candidates=[]
 for time,j in index[(u['icao24'],u['expected_df'])]:
  residual=time-target
  if abs(residual)>matching_tolerance:continue
  if u['bds']:
   field='bds'+u['bds'][2:]
   if field not in reference[j]:continue
  candidates.append((abs(residual),i,j,residual))
 if not candidates:statuses[i]='no_reference_reply';continue
 if len(candidates)>1:statuses[i]='ambiguous_reference_replies';continue
 potential.extend(candidates)
assignments={};used=set()
for error,i,j,residual in sorted(potential):
 if j in used:statuses[i]='reply_already_assigned';continue
 used.add(j);assignments[i]=(j,residual);statuses[i]='candidate_match'

fields=['uplink_utc_nominal','iq_seconds','icao24','callsign','UF','RR','BDS_query','IIS','expected_DF','DF','reply_utc_system','reply_BDS','corrected_time_residual_ms','clock_offset_s','clock_support','match_status','association_strength','uplink_raw','downlink_raw','reference_frame','source']
rr_byraw={json.loads(l)['message']['raw']:json.loads(l)['message'].get('rr','') for l in (OUT/'corrected_crc_uplinks.jsonl').open()}
rows=[]
def utc(epoch):return datetime.datetime.fromtimestamp(epoch,datetime.timezone.utc).isoformat(timespec='microseconds')
for i,u in enumerate(ups):
 model=u['_model'];row=dict.fromkeys(fields,'')
 row.update({'uplink_utc_nominal':utc(START+u['time']),'iq_seconds':f"{u['time']:.9f}",'icao24':u['icao24'],'callsign':refs.get(u['icao24'],{}).get('callsign',''),'UF':u['uf'],'RR':rr_byraw.get(u['raw'],''),'BDS_query':u['bds'],'IIS':u['iis'],'expected_DF':u['expected_df'] if u['expected_df'] is not None else '', 'clock_offset_s':f"{model['lag_s']:.6f}" if model['usable'] else '', 'clock_support':model['support'],'match_status':statuses[i],'uplink_raw':u['raw'],'source':u['source']})
 if i in assignments:
  j,residual=assignments[i];d=reference[j]
  row.update({'DF':d['_df'],'reply_utc_system':utc(d['timestamp']),'reply_BDS':','.join(k for k in ['bds20','bds40','bds50','bds60','bds10','bds17'] if k in d),'corrected_time_residual_ms':f'{residual*1000:.3f}','downlink_raw':d['frame'][:14 if d['_df']<16 else 28],'reference_frame':d['frame'],'association_strength':'register_constrained_candidate' if u['bds'] else 'timing_only_candidate'})
 rows.append(row)
with (OUT/'uplink_downlink_alignment.csv').open('w') as f:
 w=csv.DictWriter(f,fieldnames=fields);w.writeheader();w.writerows(rows)
summary={'total_uplinks':len(ups),'matched_candidates':len(assignments),'clock_tolerance_ms':matching_tolerance*1000,'status_counts':dict(collections.Counter(statuses.values())),'by_uf':{str(uf):{'uplinks':sum(u['uf']==uf for u in ups),'candidate_replies':sum(ups[i]['uf']==uf for i in assignments)} for uf in sorted(set(u['uf'] for u in ups))}}
# Shifted-time controls: counts before one-to-one assignment, showing the
# substantial accidental background for repetitive DF0 traffic.
controls={}
for shift in [.5,1.,1.5,2.]:
 counts=collections.Counter()
 for u in ups:
  model=u['_model']
  if u['provisional'] or not model['usable'] or u['expected_df'] is None:continue
  target=u['time']+model['lag_s']+shift
  candidates=[]
  for time,j in index[(u['icao24'],u['expected_df'])]:
   if abs(time-target)>matching_tolerance:continue
   if u['bds'] and 'bds'+u['bds'][2:] not in reference[j]:continue
   candidates.append(j)
  if len(candidates)==1:counts[str(u['uf'])]+=1
 controls[str(shift)]=dict(counts)
summary['shifted_time_controls_before_assignment']=controls
(OUT/'alignment_summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
print('clock model',[(m['center_s'],round(m['lag_s'],4) if m['lag_s'] is not None else None,m['support'],m['usable']) for m in models])

fig,ax=plt.subplots(figsize=(11,5),constrained_layout=True)
ax.scatter(calibration[:,0],calibration[:,1],s=8,alpha=.18,color='tab:blue',label='same-address / DF / BDS candidates')
for m in models:
 if m['usable']:ax.plot([m['center_s']-5,m['center_s']+5],[m['lag_s'],m['lag_s']],color='tab:red',lw=3)
ax.set(xlabel='IQ time into capture (s)',ylabel='Reference system time − nominal IQ time (s)',title='Locally calibrated receiver timestamp offset; red segments used for matching')
ax.grid(alpha=.2);ax.legend();fig.savefig(OUT/'reply_clock_alignment.png',dpi=160)
