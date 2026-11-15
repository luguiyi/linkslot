use slot::frames::{FrameSize, Frames};

fn publish(frames: &Frames, value: u8, size: usize) {
    let mut buf = frames.take_write();
    buf.clear();
    buf.resize(size, value);
    frames.publish(buf, FrameSize { w: 240, h: 160 });
}

#[test]
fn the_reader_gets_the_newest_frame_and_never_the_same_one_twice() {
    let f = Frames::new(4);
    for i in 0..50u8 {
        publish(&f, i, 4);
    }
    let latest = f.latest().expect("a published frame should be readable");
    assert_eq!(
        &latest[..],
        &[49u8; 4],
        "the reader was handed a stale frame"
    );
    drop(latest);
    assert!(
        f.latest().is_none(),
        "a consumed frame was handed out twice"
    );
}

#[test]
fn a_frame_carries_the_size_its_core_reported() {
    let f = Frames::new(4);
    let mut buf = f.take_write();
    buf.clear();
    buf.resize((256 * 224 * 4) as usize, 0);
    f.publish(buf, FrameSize { w: 256, h: 224 });

    let latest = f.latest().expect("a published frame should be readable");
    assert_eq!((latest.width(), latest.height()), (256, 224));
    assert_eq!(latest.len(), 256 * 224 * 4, "the buffer is the frame");
}

#[test]
fn buffers_are_recycled_rather_than_reallocated_per_frame() {
    let f = Frames::new(4);
    publish(&f, 1, 4);
    let held = f.latest().expect("a published frame should be readable");
    for i in 0..500u32 {
        publish(&f, i as u8, 4);
    }
    drop(held);
    publish(&f, 0, 4);
    assert!(
        f.allocated() <= 3,
        "{} buffers in flight, the pool is leaking",
        f.allocated()
    );
}
