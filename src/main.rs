use std::path::{Path, PathBuf};

use get_windows::enumerate_windows;
use gst::{
    PadProbeType,
    glib::{
        self,
        clone::Downgrade,
        object::{Cast, ObjectExt as _},
    },
    prelude::{
        ElementExt as _, ElementExtManual, GstBinExt, GstBinExtManual as _, GstObjectExt as _,
        PadExt, PadExtManual,
    },
};

pub mod get_windows;

pub trait PipeLineSrc {
    fn get_input_options() -> Vec<String>;

    fn from_option(option: &str) -> Self;

    fn to_element(self) -> Result<gst::Element, anyhow::Error>;
}

#[derive(Debug, Clone)]
pub struct WindowsSrc {
    window: get_windows::Window,
}

impl PipeLineSrc for WindowsSrc {
    fn get_input_options() -> Vec<String> {
        enumerate_windows().into_iter().map(|w| w.title).collect()
    }

    fn from_option(option: &str) -> Self {
        let windows = enumerate_windows();
        let window = windows
            .iter()
            .find(|w| w.title == option)
            .ok_or(anyhow::anyhow!("No window found with title {}", option))
            .unwrap()
            .to_owned();

        Self { window }
    }

    fn to_element(self) -> Result<gst::Element, anyhow::Error> {
        Ok(gst::ElementFactory::make("d3d12screencapturesrc")
            .name("src")
            .property_from_str("capture-api", "wgc")
            .property_from_str("window-capture-mode", "client")
            .property("window-handle", self.window.window_handle.0 as u64)
            .build()?)
    }
}

fn create_pipline(
    output_path: impl AsRef<Path>,
    src: impl PipeLineSrc,
) -> Result<gst::Pipeline, anyhow::Error> {
    let path = output_path.as_ref();

    let pipeline = gst::Pipeline::default();

    let video_src = src.to_element()?;

    let video_rate = gst::ElementFactory::make("videorate")
        .property("max-rate", 20)
        .build()?;

    let tee = gst::ElementFactory::make("tee").build()?;

    let inference_queue = gst::ElementFactory::make("queue").build()?;

    let file_queue = gst::ElementFactory::make("queue").build()?;

    file_queue.connect_closure(
        "overrun",
        false,
        glib::closure!(move |_overlay: &gst::Element| {
            println!("File queue overrun");
        }),
    );

    // infernece path
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
    // file path
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
        &video_src,
        &file_convert,
        &video_rate,
        &tee,
        &inference_queue,
        &file_queue,
        &inference_convert,
        inference_sink.upcast_ref(),
        &inference_scaler,
        &file_scaler,
        &encoder,
        &file_sink,
    ])?;

    // main pipeline
    gst::Element::link_many(&[&video_src, &video_rate, &tee])?;
    // // inference path
    // gst::Element::link_many(&[
    //     &inference_queue,
    //     &inference_convert,
    //     &inference_scaler,
    //     inference_sink.upcast_ref(),
    // ])?;
    // // file path
    gst::Element::link_many(&[
        &file_queue,
        &file_convert,
        &file_scaler,
        &encoder,
        &file_sink,
    ])?;

    // let tee_inference_pad = tee.request_pad_simple("src_%u").unwrap();
    // let queue_inference_pad = inference_queue.static_pad("sink").unwrap();
    // tee_inference_pad.link(&queue_inference_pad)?;
    let tee_file_pad = tee.request_pad_simple("src_%u").unwrap();
    let queue_file_pad = file_queue.static_pad("sink").unwrap();
    tee_file_pad.link(&queue_file_pad)?;

    // appsink
    inference_sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(|appsink| {
                // Pull the sample in question out of the appsink's buffer.
                let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let caps = sample.caps();
                if let Some(caps) = caps {
                    println!("caps: {:?}", caps);
                } else {
                    println!("No caps");
                }
                // let buffer = sample.buffer().ok_or_else(|| {
                //     gst::element_error!(
                //         appsink,
                //         gst::ResourceError::Failed,
                //         ("Failed to get buffer from appsink")
                //     );

                //     gst::FlowError::Error
                // })?;

                // let map = buffer.map_readable().map_err(|_| {
                //     gst::element_error!(
                //         appsink,
                //         gst::ResourceError::Failed,
                //         ("Failed to map buffer readable")
                //     );

                //     gst::FlowError::Error
                // })?;

                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    Ok(pipeline)
}

fn main() -> Result<(), anyhow::Error> {
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("GST_DEBUG", "4")
    };
    let source = WindowsSrc::get_input_options()
        .into_iter()
        .find(|w| w.to_lowercase().contains("firefox"))
        .ok_or(anyhow::anyhow!(
            "No window found with title containing 'firefox'"
        ))?
        .to_owned();

    gst::init().unwrap();

    let pipeline = create_pipline("output.mp4", WindowsSrc::from_option(&source)).unwrap();

    let src = pipeline.by_name("src").unwrap();
    let src_pad = src.static_pad("src").unwrap();

    // new frame in src element
    src_pad.add_probe(PadProbeType::BUFFER, move |_, _| {
        println!("New frame in src element");
        gst::PadProbeReturn::Ok
    });

    {
        let elemnt_name = "encodebin0";
        let src = pipeline.by_name(elemnt_name).unwrap();
        let src_pad = src.static_pad("video_0").unwrap();

        // new frame in src element
        src_pad.add_probe(PadProbeType::BUFFER, move |_, _| {
            println!("New frame in {} element", elemnt_name);
            gst::PadProbeReturn::Ok
        });
    }

    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline
        .bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    ctrlc::set_handler({
        let pipeline_weak = glib::object::ObjectExt::downgrade(&pipeline);
        move || {
            println!("Ctrl-C pressed! Stopping pipeline...");
            let Some(pipeline) = pipeline_weak.upgrade() else {
                println!("Pipeline no longer exists");
                return;
            };

            let src = pipeline.by_name("src").unwrap();
            src.send_event(gst::event::Eos::new());
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
