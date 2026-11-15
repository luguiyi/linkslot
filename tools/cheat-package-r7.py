from pathlib import Path
import json,re,hashlib
r=Path('work/cheats');rows=json.loads((r/'matches.json').read_text(encoding='utf8'))
alias={'乐高星球大战':'乐高星球大战1','伊登战机':'伊登战机1','托尼·霍克职业滑板2':'托尼霍克的职业滑板2','托尼·霍克职业滑板3':'托尼霍克的职业滑板3','指环王 双塔':'指环王 双塔奇兵','捍卫战士':'壮志凌云 烈火风暴A','星球大战前传：新帝国军队':'星战前传 新帝国军队','炸弹人 锦标赛 [1]大字版':'炸弹人 传说','炸弹人 锦标赛 [2]小字版':'炸弹人 传说','点点潮流':'几何世代 点点潮流','蜡笔小新：电影王国大冒险':'蜡笔小新1 电影王国大冒险'}
exclude={'圣石小子 光与暗的大决战1':'ROM 实际为第二作，合集仅有第一作代码','圣石小子 光与暗的大决战2':'合集仅有第一作代码','美丽新世界':'ROM 为 XXL；同名金手指标注游戏1/游戏2，疑似另一作合辑，未混用'}
out=Path('outputs/Slot-R7匹配金手指');report=[];omitted=[];damaged=[]
def convert_legacy(text):
 section=''; entries=[]
 for line in text.splitlines():
  line=line.strip()
  if line.startswith('[') and line.endswith(']'): section=line[1:-1];continue
  if not section or section=='GameInfo' or '=' not in line:continue
  option,payload=line.split('=',1);codes=[]
  for run in payload.strip().strip(';').split(';'):
   tokens=[int(t.strip(),16) for t in run.split(',')];assert len(tokens)>1
   addr=tokens[0];assert 0<=addr<0x48000
   addr=addr+0x2000000 if addr<0x40000 else addr+0x3000000-0x40000
   for offset,value in enumerate(tokens[1:]):
    assert 0<=value<=255
    codes.append(f'{0x30000000|addr+offset:08X} {value:04X}')
  entries.append((section if option=='ON' else section+'：'+option,'+'.join(codes)))
 return entries

def prepare(text,core,name):
 if re.search(r'cheat\d+_code',text):
  fields={}
  for line in text.splitlines():
   m=re.match(r'cheat(\d+)_(desc|code)\s*=\s*(.*)',line)
   if m:fields.setdefault(int(m[1]),{})[m[2]]=m[3].strip().strip('"')
  entries=[(v.get('desc','未命名金手指'),v['code']) for _,v in sorted(fields.items()) if 'code' in v]
 else:entries=convert_legacy(text)
 kept=[]
 for entry_index,(desc,code) in enumerate(entries):
  if '�' in desc:
   damaged.append({'rom':name,'core':core,'index':entry_index,'original_description':desc})
   desc=f'原文件标题损坏（条目{entry_index+1}）'
  desc=re.sub(r'^direct_v1\s+', '', desc)
  tokens=re.split(r'[+\s]+',code.strip());reason=''
  if len(tokens)%2 or any(not re.fullmatch(r'[0-9a-fA-F]{8}',v) if i%2==0 else not re.fullmatch(r'[0-9a-fA-F]{4}(?:[0-9a-fA-F]{4})?',v) for i,v in enumerate(tokens)):reason='代码字数格式异常'
  if not reason and core=='gpsp':
   if len(tokens)//2>64:reason='gpSP 每项最多64行；请使用 mGBA 对应条目'
   elif any(len(tokens[i+1])!=4 or tokens[i][0]=='9' for i in range(0,len(tokens),2)):reason='gpSP 不支持此编码；请使用 mGBA 对应条目'
  if reason:omitted.append({'rom':name,'core':core,'description':desc,'reason':reason});continue
  normalized='+'.join(tokens[i]+' '+tokens[i+1] for i in range(0,len(tokens),2))
  assert len(normalized)<1048576
  kept.append((desc.replace('"',"'"),normalized))
 assert kept,name
 return 'cheats = '+str(len(kept))+'\n\n'+'\n\n'.join(f'cheat{i}_desc = "{d}"\ncheat{i}_code = "{c}"\ncheat{i}_enable = false\ncheat{i}_handler = 0' for i,(d,c) in enumerate(kept))+'\n'

for x in rows:
 if x['rom'] in alias:x['source']=str(Path('D:/Desktop/cheats/mGBA')/(alias[x['rom']]+'.cht'))
 if x['rom'] in exclude:x['source']=None
 if not x['source']:x['status']=exclude.get(x['rom'],'合集无可确认对应文件');continue
 x['status']='同名/译名核对；代码效果与 ROM 地区版本仍需实测'
 for core,folder in [('mgba','mGBA'),('gpsp','gpSP')]:
  source=Path('D:/Desktop/cheats')/folder/Path(x['source']).name
  raw=source.read_bytes()
  try:s=raw.decode('utf-8-sig')
  except UnicodeError:s=raw.decode('gb18030')
  s=prepare(s,core,x['rom'])
  target=out/'Cheats/GBA'/core/(x['rom']+'.cht');target.parent.mkdir(parents=True,exist_ok=True);target.write_text(s,encoding='utf8')
  report.append({'rom':x['rom'],'core':core,'source':str(source),'destination':str(target.relative_to(out)),'entries':len(re.findall(r'cheat\d+_code\s*=',s))})
(r/'selected.json').write_text(json.dumps(rows,ensure_ascii=False,indent=2),encoding='utf8')
(out/'配对清单.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf8')
(out/'未匹配清单.txt').write_text('\n'.join(x['rom']+'：'+x['status'] for x in rows if not x['source']),encoding='utf8')
print('matched',sum(bool(x['source']) for x in rows),'unmatched',sum(not x['source'] for x in rows),'files',len(report),'codes',sum(x['entries'] for x in report))

(out/'未接入条目.json').write_text(json.dumps(omitted,ensure_ascii=False,indent=2),encoding='utf8')
print('omitted entries',len(omitted))
(out/'原文件标题损坏.json').write_text(json.dumps(damaged,ensure_ascii=False,indent=2),encoding='utf8')
