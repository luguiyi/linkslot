from pathlib import Path
import sys,json
r=Path(__file__).parent.parent;sys.path.insert(0,str(r/'python-tools'))
from fontTools import subset
from fontTools.ttLib import TTFont
source=r/'NotoSansCJKsc-Regular.otf'
dest=r/'crates/slot-ui/assets/SlotSimplifiedChinese.otf'
chars=set(chr(i) for i in range(0x20,0x250))
for a in range(0xa1,0xf8):
    for b in range(0xa1,0xff):
        try: chars.update(bytes([a,b]).decode('gb2312'))
        except UnicodeDecodeError: pass
for p in (r/'crates').rglob('*.rs'):
    chars.update(c for c in p.read_text(encoding='utf8') if ord(c)>127)
chars.update(chr(i) for i in range(0x2000,0x2070))
font=TTFont(source); available=set(font.getBestCmap()); wanted={ord(c) for c in chars}&available
opt=subset.Options();opt.layout_features=[];opt.name_IDs=['*'];opt.name_legacy=True;opt.name_languages=['*']
s=subset.Subsetter(options=opt);s.populate(unicodes=wanted);s.subset(font)
# Give the modified font its own family and PostScript names.
names={1:'Slot Simplified Chinese',2:'Regular',3:'SlotSimplifiedChinese-Regular-r2',4:'Slot Simplified Chinese Regular',6:'SlotSimplifiedChinese-Regular',16:'Slot Simplified Chinese',17:'Regular'}
for n in font['name'].names:
    if n.nameID in names:n.string=names[n.nameID].encode(n.getEncoding())
if 'CFF ' in font:
    cff=font['CFF '].cff;cff.fontNames=['SlotSimplifiedChinese-Regular'];top=cff.topDictIndex[0]
    top.FamilyName='Slot Simplified Chinese';top.FullName='Slot Simplified Chinese Regular'
font.save(dest)
check=TTFont(dest); assert wanted <= set(check.getBestCmap())
report={'original_bytes':source.stat().st_size,'subset_bytes':dest.stat().st_size,'glyphs':check['maxp'].numGlyphs,'codepoints':len(wanted)}
(r/'font-subset-r2.json').write_text(json.dumps(report,indent=2));print(report)
