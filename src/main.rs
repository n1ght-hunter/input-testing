use gst::{
    glib::{clone::Downgrade, object::Cast as _},
    prelude::{
        ElementExt as _, ElementExtManual as _, GstBinExt as _, GstBinExtManual as _,
        GstObjectExt as _, PadExt as _, PadExtManual as _,
    },
};

fn main() -> anyhow::Result<()> {
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("GST_DEBUG", "3")
    };

    gst::init()?;

    let path = "test.mp4";
    let window_title = "firefox";

    let pipeline = gst::Pipeline::default();

    let source = input_testing::get_windows::enumerate_windows()
        .into_iter()
        .find(|w| w.title.to_lowercase().contains(window_title))
        .ok_or(anyhow::anyhow!(
            "No window found with title containing '{window_title}'"
        ))?
        .to_owned();

    let src = gst::ElementFactory::make("d3d11screencapturesrc")
        .name("src")
        .property_from_str("capture-api", "wgc")
        .property_from_str("window-capture-mode", "client")
        .property("window-handle", source.window_handle.0 as u64)
        .build()?;

    // test src
    // let src = gst::ElementFactory::make("videotestsrc")
    //     .name("src")
    //     .property("is-live", true)
    //     .build()?;

    let video_rate = gst::ElementFactory::make("videorate")
        .property("max-rate", 20)
        .build()?;

    let tee = gst::ElementFactory::make("tee").build()?;

    // inference queue
    let inference_queue = gst::ElementFactory::make("queue").build()?;

    let inference_convert = gst::ElementFactory::make("videoconvert").build()?;
    let inference_scaler = gst::ElementFactory::make("videoscale").build()?;
    let inference_sink = gst_app::AppSink::builder()
        .caps(
            &gst_video::VideoCapsBuilder::new()
                .format(gst_video::VideoFormat::Rgb)
                .width(192)
                .height(192)
                .build(),
        )
        .build();

    // file queue
    let file_queue = gst::ElementFactory::make("queue").build()?;

    let file_convert = gst::ElementFactory::make("videoconvert").build()?;
    let file_scaler = gst::ElementFactory::make("videoscale").build()?;

    let video_profile = gst_pbutils::EncodingVideoProfile::builder(
        &gst_video::VideoCapsBuilder::for_encoding("video/x-h264").build(),
    )
    .restriction(
        &gst_video::VideoCapsBuilder::new()
            .width(640)
            .height(480)
            .build(),
    )
    .build();

    let container_profile = gst_pbutils::EncodingContainerProfile::builder(
        &gst::Caps::builder("video/quicktime").build(),
    )
    .add_profile(video_profile)
    .build();

    let encoder = gst::ElementFactory::make("encodebin")
        .property("profile", &container_profile)
        .build()?;

    let file_sink = gst::ElementFactory::make("filesink")
        .property("location", &path)
        .build()?;

    pipeline.add_many(&[
        &src,
        &video_rate,
        &file_convert,
        &file_scaler,
        &encoder,
        &file_sink,
        &tee,
        &file_queue,
        &inference_queue,
        &inference_convert,
        &inference_scaler,
        inference_sink.upcast_ref(),
    ])?;

    gst::Element::link_many(&[&src, &video_rate, &tee])?;

    // file path
    gst::Element::link_many(&[
        &file_queue,
        &file_convert,
        &file_scaler,
        &encoder,
        &file_sink,
    ])?;

    // inference path
    gst::Element::link_many(&[
        &inference_queue,
        &inference_convert,
        &inference_scaler,
        inference_sink.upcast_ref(),
    ])?;

    let tee_inference_pad = tee.request_pad_simple("src_%u").unwrap();
    let queue_inference_pad = inference_queue.static_pad("sink").unwrap();
    tee_inference_pad.link(&queue_inference_pad)?;
    let tee_file_pad = tee.request_pad_simple("src_%u").unwrap();
    let queue_file_pad = file_queue.static_pad("sink").unwrap();
    tee_file_pad.link(&queue_file_pad)?;

    inference_sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(|_appsink| {
                println!("New sample in inference sink");

                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let src = pipeline.by_name("src").unwrap();
    let src_pad = src.static_pad("src").unwrap();

    // new frame in src element
    src_pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
        println!("New frame in src element");
        gst::PadProbeReturn::Ok
    });

    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline
        .bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    ctrlc::set_handler({
        let pipeline_weak = pipeline.downgrade();
        move || {
            println!("Ctrl-C pressed! Stopping pipeline...");
            let Some(pipeline) = pipeline_weak.upgrade() else {
                println!("Pipeline no longer exists");
                return;
            };

            let src = pipeline.by_name("src").unwrap();
            src.send_event(gst::event::Eos::new());
            println!("send eos to src");
        }
    })
    .unwrap();

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => {
                println!("received eos");
                break;
            }
            MessageView::Error(err) => {
                pipeline.set_state(gst::State::Null)?;
                return Err(anyhow::anyhow!(
                    "Error received from element {}: {}",
                    err.src()
                        .map(|s| s.path_string())
                        .unwrap_or_else(|| "None".into()),
                    err.error()
                ));
            }
            _ => (),
        }
    }

    pipeline.set_state(gst::State::Null)?;

    Ok(())
}
