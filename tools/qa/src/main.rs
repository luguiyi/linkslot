use std::ffi::{c_void,CString};
use slot_gfx::{Surface,GfxError,Compositor,ScreenEffect,OUT_W,OUT_H,SRC_W,SRC_H};
use libloading::Library;
struct Angle { egl: Library, gles: Library }
impl Surface for Angle {
    fn make_current(&mut self)->Result<(),GfxError>{Ok(())}
    fn window_size(&self)->(u32,u32){(OUT_W,OUT_H)}
    fn swap(&mut self)->Result<(),GfxError>{Ok(())}
    fn proc_address(&self,name:&str)->*const c_void {
        unsafe {
            if let Ok(f)=self.gles.get::<unsafe extern "system" fn()>(name.as_bytes()){return *f as *const c_void;}
            let get=self.egl.get::<unsafe extern "system" fn(*const i8)->*const c_void>(b"eglGetProcAddress").unwrap();
            get(CString::new(name).unwrap().as_ptr())
        }
    }
}
fn png(path:&str,w:u32,h:u32,pixels:&[u8]){
    let f=std::fs::File::create(path).unwrap(); let mut e=png::Encoder::new(f,w,h);
    e.set_color(png::ColorType::Rgba);e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(pixels).unwrap();
}
fn stamp(dst:&mut [u8],dw:u32,src:&[u8],w:u32,h:u32,x:u32,y:u32){
    for sy in 0..h {for sx in 0..w { let si=((sy*w+sx)*4) as usize;let di=(((sy+y)*dw+sx+x)*4) as usize;
        if sx+x>=dw||di+3>=dst.len(){continue;} let a=src[si+3] as u32;
        for c in 0..3 { dst[di+c]=((src[si+c] as u32*a+dst[di+c] as u32*(255-a))/255) as u8; }
    }}
}
fn ui(){
    let mut image=vec![28u8;(OUT_W*1280*4) as usize];for p in image.chunks_exact_mut(4){p[3]=255;}
    for (i,row) in slot_ui::QuickRow::ALL.iter().enumerate(){
        let f=slot_ui::quick_label_face(*row);assert!(f.w>0&&f.h>0);assert!(f.w<OUT_W);
        assert!(f.rgba.chunks_exact(4).any(|p|p[3]>128));
        assert!(f.rgba.chunks_exact(4).take(f.w as usize).all(|p|p[3]==0),"top clipping {row:?}");
        assert!(f.rgba.chunks_exact(4).skip(((f.h-1)*f.w) as usize).all(|p|p[3]==0),"bottom clipping {row:?}");
        stamp(&mut image,OUT_W,&f.rgba,f.w,f.h,20,10+i as u32*48);
    }
    let f=slot_ui::quick_value_face(slot_ui::QuickValue::FourPass.text(),true);
    stamp(&mut image,OUT_W,&f.rgba,f.w,f.h,420,300);
    for (i,t) in slot_ui::Toast::all().iter().enumerate().take(16){
        let f=slot_ui::toast_face(*t); assert!(f.w>0&&f.h>0);
        stamp(&mut image,OUT_W,&f.rgba,f.w,f.h,20,720+i as u32*28);
    }
    png("../qa-ui.png",OUT_W,1280,&image);
    let f=slot_ui::sticker_face(&slot_ui::StickerFields{battery:Some(85),serial:"160C4",dirty_digit:'1'});
    png("../qa-about.png",f.w,f.h,&f.rgba);
    let dir=std::path::Path::new("../qa-card");std::fs::create_dir_all(dir.join("Config")).unwrap();
    let mut s=slot_store::SlotState::default();s.shader_gba=slot_store::Shader::FourPass;s.shader_gb=slot_store::Shader::FourPass;
    slot_store::write_slot_state(dir,&s).unwrap();let saved=slot_store::read_slot_state(dir);assert_eq!(saved,s);
    assert_eq!(slot_store::Shader::parse("grid"),Some(slot_store::Shader::Grid));
    println!("PASS: all menu labels rasterized without vertical clipping; four-pass selection persists; legacy grid parses");
}
fn gpu(){unsafe{
    let base=std::env::var("SLOT_TEST_ANGLE").unwrap();
    let a=Angle{gles:Library::new(format!("{base}/libGLESv2.dll")).unwrap(),egl:Library::new(format!("{base}/libEGL.dll")).unwrap()};
    let display=a.egl.get::<unsafe extern "system" fn(*const c_void)->*mut c_void>(b"eglGetDisplay").unwrap()(std::ptr::null());
    let init=a.egl.get::<unsafe extern "system" fn(*mut c_void,*mut i32,*mut i32)->u32>(b"eglInitialize").unwrap();
    let(mut major,mut minor)=(0,0);assert_ne!(init(display,&mut major,&mut minor),0,"EGL initialize");
    let attrs=[0x3033,1,0x3040,4,0x3024,8,0x3023,8,0x3022,8,0x3038];
    let(mut cfg,mut n)=(std::ptr::null_mut(),0);
    assert_ne!(a.egl.get::<unsafe extern "system" fn(*mut c_void,*const i32,*mut *mut c_void,i32,*mut i32)->u32>(b"eglChooseConfig").unwrap()(display,attrs.as_ptr(),&mut cfg,1,&mut n),0);assert!(n>0);
    let pa=[0x3057,720,0x3056,480,0x3038];
    let surface=a.egl.get::<unsafe extern "system" fn(*mut c_void,*mut c_void,*const i32)->*mut c_void>(b"eglCreatePbufferSurface").unwrap()(display,cfg,pa.as_ptr());assert!(!surface.is_null());
    let ca=[0x3098,2,0x3038];
    let ctx=a.egl.get::<unsafe extern "system" fn(*mut c_void,*mut c_void,*mut c_void,*const i32)->*mut c_void>(b"eglCreateContext").unwrap()(display,cfg,std::ptr::null_mut(),ca.as_ptr());assert!(!ctx.is_null());
    assert_ne!(a.egl.get::<unsafe extern "system" fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void)->u32>(b"eglMakeCurrent").unwrap()(display,surface,surface,ctx),0);
    let mut c=Compositor::new(&a).expect("compile actual GLES shaders and create FBOs");
    gl::load_with(|name|a.proc_address(name));
    println!("GLES: {}",std::ffi::CStr::from_ptr(gl::GetString(gl::VERSION).cast()).to_string_lossy());
    let mut pixels=vec![0;(SRC_W*SRC_H*4) as usize];
    for y in 0..SRC_H {for x in 0..SRC_W {let rgb=match (x<SRC_W/2,y<SRC_H/2){(true,true)=>[255,0,0],(false,true)=>[0,255,0],(true,false)=>[0,0,255],_=>[240,240,240]};let at=((y*SRC_W+x)*4) as usize;pixels[at..at+4].copy_from_slice(&[rgb[2],rgb[1],rgb[0],255]);}}
    c.upload_game(&pixels);c.set_screen_power(1.0);
    let mut frames=Vec::new();
    for (name,effect) in [("plain",ScreenEffect::None),("four-pass",ScreenEffect::FourPass),("plain-again",ScreenEffect::None)]{
        c.set_screen_effect(effect);c.begin_frame();c.draw_game();let p=c.read_frame();assert_eq!(gl::GetError(),gl::NO_ERROR,"GL error in {name}");
        png(&format!("../qa-{name}.png"),OUT_W,OUT_H,&p);frames.push(p);
    }
    assert_eq!(frames[0],frames[2],"switching back must restore original render state");assert_ne!(frames[0],frames[1]);
    let pixel=|x,y| {let i=((y*OUT_W+x)*4) as usize; &frames[1][i..i+4]};
    let red=pixel(90,60);let green=pixel(540,60);let blue=pixel(90,360);
    assert!(red[0]>red[1]&&red[0]>red[2],"top-left stays red: {red:?}");
    assert!(green[1]>green[0]&&green[1]>green[2],"top-right stays green: {green:?}");
    assert!(blue[2]>blue[0]&&blue[2]>blue[1],"bottom-left stays blue: {blue:?}");
    println!("PASS: four GLES programs compiled; real four-pass output changes image, preserves orientation; switching off restores exact original frame; no GL errors");
    c.fit((640,480));c.set_screen_effect(ScreenEffect::FourPass);c.begin_frame();c.draw_game();
    let (w,h)=slot_gfx::canvas_size((640,480));let frame=c.read_frame();assert_eq!(frame.len(),(w*h*4) as usize);assert_eq!(gl::GetError(),0);png("../qa-four-pass-640.png",w,h,&frame);
    c.fit((720,480));
    let upload=|c:&mut Compositor,f:slot_ui::UndoFace|{(c.create_texture(f.w,f.h,&f.rgba),f.w,f.h)};
    let labels=slot_ui::QuickRow::ALL.iter().map(|r|upload(&mut c,slot_ui::quick_label_face(*r))).collect();
    let values=slot_ui::QuickValue::ALL.iter().map(|v|[upload(&mut c,slot_ui::quick_value_face(v.text(),false)),upload(&mut c,slot_ui::quick_value_face(v.text(),true))]).collect();
    let carets=[upload(&mut c,slot_ui::quick_caret_face(false)),upload(&mut c,slot_ui::quick_caret_face(true))];
    let legend=slot_ui::quick_legend_faces().map(|f|{let(t,w,_)=upload(&mut c,f);(t,w)});
    let faces=slot_ui::QuickMenuFaces{labels,values,carets,legend};
    let mut values=[None;slot_ui::QuickRow::ALL.len()];
    values[slot_ui::QuickRow::GbaShader.index()]=Some(slot_ui::QuickValue::FourPass);
    values[slot_ui::QuickRow::GbShader.index()]=Some(slot_ui::QuickValue::FourPass);
    values[slot_ui::QuickRow::ColourCorrection.index()]=Some(slot_ui::QuickValue::Off);
    values[slot_ui::QuickRow::GbPalettes.index()]=Some(slot_ui::QuickValue::Off);
    let menu=slot_ui::QuickMenu{row:slot_ui::QuickRow::GbaShader,values,clock:None,faces:Some(&faces)};
    let mut draws=Vec::new();menu.draw(&mut draws);c.begin_frame();c.draw_list(&draws);png("../qa-menu.png",720,480,&c.read_frame());assert_eq!(gl::GetError(),0);
    for (name, value) in [("both",slot_ui::QuickValue::Both),("swap",slot_ui::QuickValue::SwapShoulders),("xy",slot_ui::QuickValue::XyShoulders),("off",slot_ui::QuickValue::Off)] {
        let mut values=[None;slot_ui::QuickRow::ALL.len()];
        for row in slot_ui::QuickRow::GAME {values[row.index()]=Some(slot_ui::QuickValue::On);}
        values[slot_ui::QuickRow::FastForward.index()]=Some(slot_ui::QuickValue::Speed6);
        values[slot_ui::QuickRow::Turbo.index()]=Some(slot_ui::QuickValue::Off);
        values[slot_ui::QuickRow::ButtonLayout.index()]=Some(value);
        let menu=slot_ui::QuickMenu{row:slot_ui::QuickRow::ButtonLayout,values,clock:None,faces:Some(&faces)};
        let mut draws=Vec::new();menu.draw(&mut draws);
        c.begin_frame();c.draw_list(&draws);png(&format!("../qa-r3-{name}.png"),720,480,&c.read_frame());
        assert_eq!(gl::GetError(),0);
    }
    for (name,value) in [("3",slot_ui::QuickValue::Minutes3),("10",slot_ui::QuickValue::Minutes10),("30",slot_ui::QuickValue::Minutes30),("never",slot_ui::QuickValue::Never)] {
        let mut values=[None;slot_ui::QuickRow::ALL.len()];values[slot_ui::QuickRow::LidTimeout.index()]=Some(value);
        let menu=slot_ui::QuickMenu{row:slot_ui::QuickRow::LidTimeout,values,clock:None,faces:Some(&faces)};
        let mut draws=Vec::new();menu.draw(&mut draws);c.begin_frame();c.draw_list(&draws);png(&format!("../qa-r7-settings-{name}.png"),720,480,&c.read_frame());assert_eq!(gl::GetError(),0);
    }
    println!("PASS: noninteger 640-panel canvas render; actual Chinese settings menu rendered with four-pass option");
    std::env::set_var("SLOT_TEST_FOUR_PASS_ERROR", "1");
    let mut fallback=Compositor::new(&a).expect("a broken optional shader must not prevent compositor startup");
    fallback.upload_game(&pixels);fallback.set_screen_power(1.0);
    fallback.set_screen_effect(ScreenEffect::FourPass);fallback.begin_frame();fallback.draw_game();
    assert_eq!(fallback.read_frame(),frames[0],"failed shader falls back to unfiltered output");
    fallback.begin_frame();fallback.draw_game();assert_eq!(gl::GetError(),0);
    std::env::remove_var("SLOT_TEST_FOUR_PASS_ERROR");
    println!("PASS: injected optional shader failure does not prevent startup; falls back to exact unfiltered frame");
}}
mod r3;
fn main(){ui();r3::run();navigation_qa();r6_qa();r7_qa();r8_qa();gpu();}


fn navigation_qa() {
    use slot_store::title_search::initial;
    for (s,c) in [("口袋妖怪",'K'),("火焰纹章",'H'),("恶魔城",'E'),("最终幻想",'Z'),("  mario",'M'),("（塞尔达）",'S')] { assert_eq!(initial(s),Some(c)); }
    assert_eq!(initial("123"),None); assert_eq!(initial(""),None);
    let fallback=slot_ui::navigation::title("铠");assert!(fallback.rgba.chunks_exact(4).any(|p|p[3]>0));
    let long="火焰纹章 封印之剑 末世余烬";
    let f=slot_ui::navigation::title(long);assert_eq!((f.w,f.h),(664,76));assert!(f.rgba.chunks_exact(4).any(|p|p[3]>0));png("../qa-r5-title.png",f.w,f.h,&f.rgba);
    let available=std::array::from_fn(|i|[4,7,10,12,25].contains(&i));
    let f=slot_ui::navigation::picker(10,available);png("../qa-r5-picker.png",f.w,f.h,&f.rgba);
    for flag in [false,true] {
        let dir=std::path::Path::new("../qa-card");let mut state=slot_store::SlotState::default();state.show_rom_title=flag;
        slot_store::write_slot_state(dir,&state).unwrap();assert_eq!(slot_store::read_slot_state(dir),state);
    }
    assert_eq!(slot_ui::QuickRow::RomTitle.parent(),Some(slot_ui::QuickRow::Screen));
    println!("PASS R5: pinyin and ASCII initials, empty/numeric names, supplied font rasterization, title toggle persistence and menu placement");
}

fn r6_qa() {
    use slot_store::{scan, initial, sort_key};
    let root=std::path::Path::new("../qa-r6-card");
    for folder in ["Games/GBA", "Games/GBA/改版", "Labels/GBA", "Labels/GBA/改版"] {std::fs::create_dir_all(root.join(folder)).unwrap();}
    let names:Vec<_>=include_str!("../rom-names-r6.txt").lines().collect();
    assert_eq!(names.len(),289);
    for (i,name) in names.iter().enumerate() {
        let folder=if i%2==0 {"Games/GBA"} else {"Games/GBA/改版"};
        let ext=if i%3==0 {"GBA"} else {"gba"};
        std::fs::write(root.join(format!("{folder}/{name}.{ext}")),[]).unwrap();
    }
    std::fs::write(root.join("Games/GBA/._hidden.gba"),[]).unwrap();
    std::fs::write(root.join("Games/GBA/unsupported.zip"),[]).unwrap();
    std::fs::write(root.join("Labels/GBA/星之卡比1 梦之泉.PNG"),[]).unwrap();
    let carts=scan(root).unwrap();assert_eq!(carts.len(),289);
    assert!(carts.windows(2).all(|w|sort_key(&w[0].stem)<=sort_key(&w[1].stem)));
    for name in ["星之卡比1 梦之泉","星之卡比2 镜之大迷宫"] {
        let c=carts.iter().find(|c|c.stem==name).expect("Kirby is scanned");
        assert_eq!(initial(&c.stem),'X');
    }
    assert!(carts.iter().find(|c|c.stem=="星之卡比1 梦之泉").unwrap().label.is_some());
    let expected:Vec<_>=carts.iter().map(|c|initial(&c.stem)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let mut shelf=slot_ui::Shelf::new(carts);
    let mut got=vec![initial(&shelf.carts[shelf.index].stem)];
    for _ in 1..expected.len() {shelf.jump_next_letter();got.push(initial(&shelf.carts[shelf.index].stem));}
    assert_eq!(got,expected);shelf.jump_next_letter();assert_eq!(shelf.index,0);
    shelf.jump_prev_letter();assert_eq!(initial(&shelf.carts[shelf.index].stem),*expected.last().unwrap());
    let first_x=shelf.carts.iter().position(|c|slot_store::title_search::initial(&c.stem)==Some('X')).unwrap();
    shelf.select(first_x);shelf.jump_next_letter();assert_eq!(initial(&shelf.carts[shelf.index].stem),'Y');
    shelf.jump_prev_letter();assert_eq!(shelf.index,first_x);
    assert!(std::fs::read_to_string(root.join("slot-library.log")).unwrap().contains("星之卡比2 镜之大迷宫"));
    println!("PASS R6: all 289 names scanned across flat/nested folders, upper/lower extensions, hidden/ZIP excluded, Kirby in X, up/down and picker share contiguous groups, scan report written");
}

fn r7_qa() {
    use slot_store::cheats::{parse,lid_expired};
    let rows=parse("cheats = 2\ncheat0_desc = \"金钱\"\ncheat0_code = \"8203FFF0+1234\"\ncheat0_enable = true\ncheat1_code=bad<script>\n");
    assert_eq!(rows.len(),1);assert!(!rows[0].enabled);
    assert!(parse("cheat0_code=82000000+0001\ncheat0_handler=1").is_empty());
    for minutes in [3,10,30] {assert!(!lid_expired(minutes,u64::from(minutes)*60_000-1));assert!(lid_expired(minutes,u64::from(minutes)*60_000));}
    assert!(!lid_expired(0,u64::MAX));
    let dir=std::path::Path::new("../qa-card");
    for minutes in [3,10,30,0] {let mut state=slot_store::SlotState::default();state.lid_minutes=minutes;slot_store::write_slot_state(dir,&state).unwrap();assert_eq!(slot_store::read_slot_state(dir).lid_minutes,minutes);}
    let state_path=dir.join("linkSlot/Config/slot.state");
    let text=std::fs::read_to_string(&state_path).unwrap();
    std::fs::write(&state_path,text.replace("lid_minutes=0\n", "")).unwrap();assert_eq!(slot_store::read_slot_state(dir).lid_minutes,3);
    std::fs::write(&state_path,text.replace("lid_minutes=0", "lid_minutes=7")).unwrap();assert_eq!(slot_store::read_slot_state(dir).lid_minutes,3);
    let cheat_root=std::env::var("SLOT_CHEAT_TEST_ROOT").expect("Set SLOT_CHEAT_TEST_ROOT to extracted Cheats/GBA");
    let mut count=0;
    for core in ["mgba","gpsp"] {
        for entry in std::fs::read_dir(format!("{cheat_root}/{core}")).unwrap() {
            let text=std::fs::read_to_string(entry.unwrap().path()).unwrap();let codes=parse(&text);
            assert!(!codes.is_empty());assert!(codes.iter().all(|c|!c.enabled));count+=1;
        }
    }
    assert_eq!(count,498);
    let help=slot_ui::navigation::HELP.iter().map(|s|s.to_string()).collect::<Vec<_>>();
    for i in [0,6,12,18,24,30] {let face=slot_ui::navigation::panel("快捷键一览",&help,i);png(&format!("../qa-r7-help-{i}.png"),face.w,face.h,&face.rgba);}
    let face=slot_ui::navigation::panel("金手指 · mGBA",&["[关] direct_v1 金钱全满".into(),"[开] 生命无限".into()],1);png("../qa-r7-cheats.png",face.w,face.h,&face.rgba);
    println!("PASS R7: four timeout boundaries including never, setting persistence, 498 cheat files parsed with all entries disabled, handler rejection, help and cheat UI rasterized");
}

fn r8_qa() {
    use slot_input::{Gestures,RawEvent,Btn,Action,MENU_HOLD_MS};
    let mut g=Gestures::new();
    for at in [0,200,400] {assert!(g.feed(RawEvent::Down(Btn::Menu),at).is_empty());assert_eq!(g.feed(RawEvent::Up(Btn::Menu),at+50),vec![Action::QuickMenu]);}
    assert!(g.feed(RawEvent::Down(Btn::Menu),600).is_empty());assert_eq!(g.tick(600+MENU_HOLD_MS),vec![Action::Eject]);assert!(g.feed(RawEvent::Up(Btn::Menu),1700).is_empty());
    assert_eq!(slot_ui::QuickValue::Minutes3.text(),"3分钟");
    let face=slot_ui::navigation::panel("游戏菜单",&["即时存档相册".into(),"金手指".into(),"快捷键一览".into(),"返回游戏".into()],0);png("../qa-r8-game-menu.png",face.w,face.h,&face.rgba);
    let empty=slot_ui::navigation::panel("即时存档相册",&[],0);png("../qa-r9-empty-album.png",empty.w,empty.h,&empty.rgba);
    println!("PASS R8: repeated M taps only produce regular menu actions, hold M still ejects, updated timeout label and game menu rendered");
}
