/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#[macro_use]
extern crate log;
#[macro_use]
pub extern crate wgpu_core as wgpu;
pub extern crate wgpu_types as wgt;

pub mod identity;

use arrayvec::ArrayVec;
use identity::{IdentityRecyclerFactory, WebGPUMsg};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};
use serde::{Deserialize, Serialize};
use servo_config::pref;
use smallvec::SmallVec;
use std::ffi::CString;
use std::ptr;
use wgpu::{
    binding_model::{
        BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor, BindGroupLayoutEntry,
    },
    id,
    instance::RequestAdapterOptions,
};

#[derive(Debug, Deserialize, Serialize)]
pub enum WebGPUResponse {
    RequestAdapter {
        adapter_name: String,
        adapter_id: WebGPUAdapter,
        channel: WebGPU,
    },
    RequestDevice {
        device_id: WebGPUDevice,
        queue_id: WebGPUQueue,
        _descriptor: wgt::DeviceDescriptor,
    },
}

pub type WebGPUResponseResult = Result<WebGPUResponse, String>;

#[derive(Debug, Deserialize, Serialize)]
pub enum WebGPURequest {
    CommandEncoderFinish {
        command_encoder_id: id::CommandEncoderId,
        // TODO(zakorgy): Serialize CommandBufferDescriptor in wgpu-core
        // wgpu::command::CommandBufferDescriptor,
    },
    CopyBufferToBuffer {
        command_encoder_id: id::CommandEncoderId,
        source_id: id::BufferId,
        source_offset: wgt::BufferAddress,
        destination_id: id::BufferId,
        destination_offset: wgt::BufferAddress,
        size: wgt::BufferAddress,
    },
    CreateBindGroup {
        device_id: id::DeviceId,
        bind_group_id: id::BindGroupId,
        bind_group_layout_id: id::BindGroupLayoutId,
        bindings: Vec<BindGroupEntry>,
    },
    CreateBindGroupLayout {
        device_id: id::DeviceId,
        bind_group_layout_id: id::BindGroupLayoutId,
        bindings: Vec<BindGroupLayoutEntry>,
    },
    CreateBuffer {
        device_id: id::DeviceId,
        buffer_id: id::BufferId,
        descriptor: wgt::BufferDescriptor<String>,
    },
    CreateCommandEncoder {
        device_id: id::DeviceId,
        // TODO(zakorgy): Serialize CommandEncoderDescriptor in wgpu-core
        // wgpu::command::CommandEncoderDescriptor,
        command_encoder_id: id::CommandEncoderId,
    },
    CreateComputePipeline {
        device_id: id::DeviceId,
        compute_pipeline_id: id::ComputePipelineId,
        pipeline_layout_id: id::PipelineLayoutId,
        program_id: id::ShaderModuleId,
        entry_point: String,
    },
    CreatePipelineLayout {
        device_id: id::DeviceId,
        pipeline_layout_id: id::PipelineLayoutId,
        bind_group_layouts: Vec<id::BindGroupLayoutId>,
    },
    CreateRenderPipeline {
        device_id: id::DeviceId,
        render_pipeline_id: id::RenderPipelineId,
        pipeline_layout_id: id::PipelineLayoutId,
        vertex_module: id::ShaderModuleId,
        vertex_entry_point: String,
        fragment_module: Option<id::ShaderModuleId>,
        fragment_entry_point: Option<String>,
        primitive_topology: wgt::PrimitiveTopology,
        rasterization_state: wgt::RasterizationStateDescriptor,
        color_states: ArrayVec<[wgt::ColorStateDescriptor; wgpu::device::MAX_COLOR_TARGETS]>,
        depth_stencil_state: Option<wgt::DepthStencilStateDescriptor>,
        vertex_state: (
            wgt::IndexFormat,
            Vec<(u64, wgt::InputStepMode, Vec<wgt::VertexAttributeDescriptor>)>,
        ),
        sample_count: u32,
        sample_mask: u32,
        alpha_to_coverage_enabled: bool,
    },
    CreateSampler {
        device_id: id::DeviceId,
        sampler_id: id::SamplerId,
        descriptor: wgt::SamplerDescriptor<String>,
    },
    CreateShaderModule {
        device_id: id::DeviceId,
        program_id: id::ShaderModuleId,
        program: Vec<u32>,
    },
    CreateTexture {
        device_id: id::DeviceId,
        texture_id: id::TextureId,
        descriptor: wgt::TextureDescriptor<String>,
    },
    CreateTextureView {
        texture_id: id::TextureId,
        texture_view_id: id::TextureViewId,
        descriptor: wgt::TextureViewDescriptor<String>,
    },
    DestroyBuffer(id::BufferId),
    DestroyTexture(id::TextureId),
    Exit(IpcSender<()>),
    RequestAdapter {
        sender: IpcSender<WebGPUResponseResult>,
        options: RequestAdapterOptions,
        ids: SmallVec<[id::AdapterId; 4]>,
    },
    RequestDevice {
        sender: IpcSender<WebGPUResponseResult>,
        adapter_id: WebGPUAdapter,
        descriptor: wgt::DeviceDescriptor,
        device_id: id::DeviceId,
    },
    RunComputePass {
        command_encoder_id: id::CommandEncoderId,
        pass_data: Vec<u8>,
    },
    RunRenderPass {
        command_encoder_id: id::CommandEncoderId,
        pass_data: Vec<u8>,
    },
    Submit {
        queue_id: id::QueueId,
        command_buffers: Vec<id::CommandBufferId>,
    },
    UnmapBuffer {
        device_id: id::DeviceId,
        buffer_id: id::BufferId,
        array_buffer: Vec<u8>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebGPU(pub IpcSender<WebGPURequest>);

impl WebGPU {
    pub fn new() -> Option<(Self, IpcReceiver<WebGPUMsg>)> {
        if !pref!(dom.webgpu.enabled) {
            return None;
        }
        let (sender, receiver) = match ipc::channel() {
            Ok(sender_and_receiver) => sender_and_receiver,
            Err(e) => {
                warn!(
                    "Failed to create sender and receiver for WGPU thread ({})",
                    e
                );
                return None;
            },
        };
        let sender_clone = sender.clone();

        let (script_sender, script_recv) = match ipc::channel() {
            Ok(sender_and_receiver) => sender_and_receiver,
            Err(e) => {
                warn!(
                    "Failed to create receiver and sender for WGPU thread ({})",
                    e
                );
                return None;
            },
        };

        if let Err(e) = std::thread::Builder::new()
            .name("WGPU".to_owned())
            .spawn(move || {
                WGPU::new(receiver, sender_clone, script_sender).run();
            })
        {
            warn!("Failed to spwan WGPU thread ({})", e);
            return None;
        }
        Some((WebGPU(sender), script_recv))
    }

    pub fn exit(&self, sender: IpcSender<()>) -> Result<(), &'static str> {
        self.0
            .send(WebGPURequest::Exit(sender))
            .map_err(|_| "Failed to send Exit message")
    }
}

struct WGPU {
    receiver: IpcReceiver<WebGPURequest>,
    sender: IpcSender<WebGPURequest>,
    script_sender: IpcSender<WebGPUMsg>,
    global: wgpu::hub::Global<IdentityRecyclerFactory>,
    adapters: Vec<WebGPUAdapter>,
    devices: Vec<WebGPUDevice>,
    // Track invalid adapters https://gpuweb.github.io/gpuweb/#invalid
    _invalid_adapters: Vec<WebGPUAdapter>,
}

impl WGPU {
    fn new(
        receiver: IpcReceiver<WebGPURequest>,
        sender: IpcSender<WebGPURequest>,
        script_sender: IpcSender<WebGPUMsg>,
    ) -> Self {
        let factory = IdentityRecyclerFactory {
            sender: script_sender.clone(),
        };
        WGPU {
            receiver,
            sender,
            script_sender,
            global: wgpu::hub::Global::new("wgpu-core", factory),
            adapters: Vec::new(),
            devices: Vec::new(),
            _invalid_adapters: Vec::new(),
        }
    }

    fn run(mut self) {
        while let Ok(msg) = self.receiver.recv() {
            match msg {
                WebGPURequest::CommandEncoderFinish { command_encoder_id } => {
                    let global = &self.global;
                    let _ = gfx_select!(command_encoder_id => global.command_encoder_finish(
                        command_encoder_id,
                        &wgt::CommandBufferDescriptor::default()
                    ));
                },
                WebGPURequest::CopyBufferToBuffer {
                    command_encoder_id,
                    source_id,
                    source_offset,
                    destination_id,
                    destination_offset,
                    size,
                } => {
                    let global = &self.global;
                    let _ = gfx_select!(command_encoder_id => global.command_encoder_copy_buffer_to_buffer(
                        command_encoder_id,
                        source_id,
                        source_offset,
                        destination_id,
                        destination_offset,
                        size
                    ));
                },
                WebGPURequest::CreateBindGroup {
                    device_id,
                    bind_group_id,
                    bind_group_layout_id,
                    bindings,
                } => {
                    let global = &self.global;
                    let descriptor = BindGroupDescriptor {
                        layout: bind_group_layout_id,
                        entries: bindings.as_ptr(),
                        entries_length: bindings.len(),
                        label: ptr::null(),
                    };
                    let _ = gfx_select!(bind_group_id =>
                        global.device_create_bind_group(device_id, &descriptor, bind_group_id));
                },
                WebGPURequest::CreateBindGroupLayout {
                    device_id,
                    bind_group_layout_id,
                    bindings,
                } => {
                    let global = &self.global;
                    let descriptor = BindGroupLayoutDescriptor {
                        entries: bindings.as_ptr(),
                        entries_length: bindings.len(),
                        label: ptr::null(),
                    };
                    let _ = gfx_select!(bind_group_layout_id =>
                        global.device_create_bind_group_layout(device_id, &descriptor, bind_group_layout_id));
                },
                WebGPURequest::CreateBuffer {
                    device_id,
                    buffer_id,
                    descriptor,
                } => {
                    let global = &self.global;
                    let st = CString::new(descriptor.label.as_bytes()).unwrap();
                    let _ = gfx_select!(buffer_id =>
                        global.device_create_buffer(device_id, &descriptor.map_label(|_| st.as_ptr()), buffer_id));
                },
                WebGPURequest::CreateCommandEncoder {
                    device_id,
                    command_encoder_id,
                } => {
                    let global = &self.global;
                    let _ = gfx_select!(command_encoder_id =>
                        global.device_create_command_encoder(device_id, &Default::default(), command_encoder_id));
                },
                WebGPURequest::CreateComputePipeline {
                    device_id,
                    compute_pipeline_id,
                    pipeline_layout_id,
                    program_id,
                    entry_point,
                } => {
                    let global = &self.global;
                    let entry_point = std::ffi::CString::new(entry_point).unwrap();
                    let descriptor = wgpu_core::pipeline::ComputePipelineDescriptor {
                        layout: pipeline_layout_id,
                        compute_stage: wgpu_core::pipeline::ProgrammableStageDescriptor {
                            module: program_id,
                            entry_point: entry_point.as_ptr(),
                        },
                    };
                    let _ = gfx_select!(compute_pipeline_id =>
                        global.device_create_compute_pipeline(device_id, &descriptor, compute_pipeline_id));
                },
                WebGPURequest::CreatePipelineLayout {
                    device_id,
                    pipeline_layout_id,
                    bind_group_layouts,
                } => {
                    let global = &self.global;
                    let descriptor = wgpu_core::binding_model::PipelineLayoutDescriptor {
                        bind_group_layouts: bind_group_layouts.as_ptr(),
                        bind_group_layouts_length: bind_group_layouts.len(),
                    };
                    let _ = gfx_select!(pipeline_layout_id =>
                        global.device_create_pipeline_layout(device_id, &descriptor, pipeline_layout_id));
                },
                //TODO: consider https://github.com/gfx-rs/wgpu/issues/684
                WebGPURequest::CreateRenderPipeline {
                    device_id,
                    render_pipeline_id,
                    pipeline_layout_id,
                    vertex_module,
                    vertex_entry_point,
                    fragment_module,
                    fragment_entry_point,
                    primitive_topology,
                    rasterization_state,
                    color_states,
                    depth_stencil_state,
                    vertex_state,
                    sample_count,
                    sample_mask,
                    alpha_to_coverage_enabled,
                } => {
                    let global = &self.global;
                    let vertex_ep = std::ffi::CString::new(vertex_entry_point).unwrap();
                    let frag_stage = match fragment_module {
                        Some(frag) => {
                            let frag_ep =
                                std::ffi::CString::new(fragment_entry_point.unwrap()).unwrap();
                            let frag_module = wgpu_core::pipeline::ProgrammableStageDescriptor {
                                module: frag,
                                entry_point: frag_ep.as_ptr(),
                            };
                            Some(frag_module)
                        },
                        None => None,
                    };
                    let descriptor = wgpu_core::pipeline::RenderPipelineDescriptor {
                        layout: pipeline_layout_id,
                        vertex_stage: wgpu_core::pipeline::ProgrammableStageDescriptor {
                            module: vertex_module,
                            entry_point: vertex_ep.as_ptr(),
                        },
                        fragment_stage: frag_stage
                            .as_ref()
                            .map_or(ptr::null(), |fs| fs as *const _),
                        primitive_topology,
                        rasterization_state: &rasterization_state as *const _,
                        color_states: color_states.as_ptr(),
                        color_states_length: color_states.len(),
                        depth_stencil_state: depth_stencil_state
                            .as_ref()
                            .map_or(ptr::null(), |dss| dss as *const _),
                        vertex_state: wgpu_core::pipeline::VertexStateDescriptor {
                            index_format: vertex_state.0,
                            vertex_buffers_length: vertex_state.1.len(),
                            vertex_buffers: vertex_state
                                .1
                                .iter()
                                .map(|buffer| wgpu_core::pipeline::VertexBufferLayoutDescriptor {
                                    array_stride: buffer.0,
                                    step_mode: buffer.1,
                                    attributes_length: buffer.2.len(),
                                    attributes: buffer.2.as_ptr(),
                                })
                                .collect::<Vec<_>>()
                                .as_ptr(),
                        },
                        sample_count,
                        sample_mask,
                        alpha_to_coverage_enabled,
                    };

                    let _ = gfx_select!(render_pipeline_id =>
                        global.device_create_render_pipeline(device_id, &descriptor, render_pipeline_id));
                },
                WebGPURequest::CreateSampler {
                    device_id,
                    sampler_id,
                    descriptor,
                } => {
                    let global = &self.global;
                    let st = CString::new(descriptor.label.as_bytes()).unwrap();
                    let _ = gfx_select!(sampler_id =>
                        global.device_create_sampler(device_id, &descriptor.map_label(|_| st.as_ptr()), sampler_id));
                },
                WebGPURequest::CreateShaderModule {
                    device_id,
                    program_id,
                    program,
                } => {
                    let global = &self.global;
                    let descriptor = wgpu_core::pipeline::ShaderModuleDescriptor {
                        code: wgpu_core::U32Array {
                            bytes: program.as_ptr(),
                            length: program.len(),
                        },
                    };
                    let _ = gfx_select!(program_id =>
                        global.device_create_shader_module(device_id, &descriptor, program_id));
                },
                WebGPURequest::CreateTexture {
                    device_id,
                    texture_id,
                    descriptor,
                } => {
                    let global = &self.global;
                    let st = CString::new(descriptor.label.as_bytes()).unwrap();
                    let _ = gfx_select!(texture_id =>
                        global.device_create_texture(device_id, &descriptor.map_label(|_| st.as_ptr()), texture_id));
                },
                WebGPURequest::CreateTextureView {
                    texture_id,
                    texture_view_id,
                    descriptor,
                } => {
                    let global = &self.global;
                    let st = CString::new(descriptor.label.as_bytes()).unwrap();
                    let _ = gfx_select!(texture_view_id => global.texture_create_view(
                        texture_id,
                        Some(&descriptor.map_label(|_| st.as_ptr())),
                        texture_view_id
                    ));
                },
                WebGPURequest::DestroyBuffer(buffer) => {
                    let global = &self.global;
                    gfx_select!(buffer => global.buffer_destroy(buffer));
                },
                WebGPURequest::DestroyTexture(texture) => {
                    let global = &self.global;
                    gfx_select!(texture => global.texture_destroy(texture));
                },
                WebGPURequest::Exit(sender) => {
                    if let Err(e) = self.script_sender.send(WebGPUMsg::Exit) {
                        warn!("Failed to send WebGPUMsg::Exit to script ({})", e);
                    }
                    drop(self.global);
                    if let Err(e) = sender.send(()) {
                        warn!("Failed to send response to WebGPURequest::Exit ({})", e)
                    }
                    return;
                },
                WebGPURequest::RequestAdapter {
                    sender,
                    options,
                    ids,
                } => {
                    let adapter_id = match self.global.pick_adapter(
                        &options,
                        wgpu::instance::AdapterInputs::IdSet(&ids, |id| id.backend()),
                    ) {
                        Some(id) => id,
                        None => {
                            if let Err(e) =
                                sender.send(Err("Failed to get webgpu adapter".to_string()))
                            {
                                warn!(
                                    "Failed to send response to WebGPURequest::RequestAdapter ({})",
                                    e
                                )
                            }
                            return;
                        },
                    };
                    let adapter = WebGPUAdapter(adapter_id);
                    self.adapters.push(adapter);
                    let global = &self.global;
                    let info = gfx_select!(adapter_id => global.adapter_get_info(adapter_id));
                    if let Err(e) = sender.send(Ok(WebGPUResponse::RequestAdapter {
                        adapter_name: info.name,
                        adapter_id: adapter,
                        channel: WebGPU(self.sender.clone()),
                    })) {
                        warn!(
                            "Failed to send response to WebGPURequest::RequestAdapter ({})",
                            e
                        )
                    }
                },
                WebGPURequest::RequestDevice {
                    sender,
                    adapter_id,
                    descriptor,
                    device_id,
                } => {
                    let global = &self.global;
                    let id = gfx_select!(device_id => global.adapter_request_device(
                        adapter_id.0,
                        &descriptor,
                        None,
                        device_id
                    ));

                    let device = WebGPUDevice(id);
                    // Note: (zakorgy) Note sure if sending the queue is needed at all,
                    // since wgpu-core uses the same id for the device and the queue
                    let queue = WebGPUQueue(id);
                    self.devices.push(device);
                    if let Err(e) = sender.send(Ok(WebGPUResponse::RequestDevice {
                        device_id: device,
                        queue_id: queue,
                        _descriptor: descriptor,
                    })) {
                        warn!(
                            "Failed to send response to WebGPURequest::RequestDevice ({})",
                            e
                        )
                    }
                },
                WebGPURequest::RunComputePass {
                    command_encoder_id,
                    pass_data,
                } => {
                    let global = &self.global;
                    gfx_select!(command_encoder_id => global.command_encoder_run_compute_pass(
                        command_encoder_id,
                        &pass_data
                    ));
                },
                WebGPURequest::RunRenderPass {
                    command_encoder_id,
                    pass_data,
                } => {
                    let global = &self.global;
                    gfx_select!(command_encoder_id => global.command_encoder_run_render_pass(
                        command_encoder_id,
                        &pass_data
                    ));
                },
                WebGPURequest::Submit {
                    queue_id,
                    command_buffers,
                } => {
                    let global = &self.global;
                    let _ = gfx_select!(queue_id => global.queue_submit(
                        queue_id,
                        &command_buffers
                    ));
                },
                WebGPURequest::UnmapBuffer {
                    device_id,
                    buffer_id,
                    array_buffer,
                } => {
                    let global = &self.global;

                    gfx_select!(buffer_id => global.device_set_buffer_sub_data(
                        device_id,
                        buffer_id,
                        0,
                        array_buffer.as_slice()
                    ));
                },
            }
        }
    }
}

macro_rules! webgpu_resource {
    ($name:ident, $id:ty) => {
        #[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Serialize)]
        pub struct $name(pub $id);

        impl MallocSizeOf for $name {
            fn size_of(&self, _ops: &mut MallocSizeOfOps) -> usize {
                0
            }
        }

        impl Eq for $name {}
    };
}

webgpu_resource!(WebGPUAdapter, id::AdapterId);
webgpu_resource!(WebGPUBindGroup, id::BindGroupId);
webgpu_resource!(WebGPUBindGroupLayout, id::BindGroupLayoutId);
webgpu_resource!(WebGPUBuffer, id::BufferId);
webgpu_resource!(WebGPUCommandBuffer, id::CommandBufferId);
webgpu_resource!(WebGPUCommandEncoder, id::CommandEncoderId);
webgpu_resource!(WebGPUComputePipeline, id::ComputePipelineId);
webgpu_resource!(WebGPUDevice, id::DeviceId);
webgpu_resource!(WebGPUPipelineLayout, id::PipelineLayoutId);
webgpu_resource!(WebGPUQueue, id::QueueId);
webgpu_resource!(WebGPURenderPipeline, id::RenderPipelineId);
webgpu_resource!(WebGPUSampler, id::SamplerId);
webgpu_resource!(WebGPUShaderModule, id::ShaderModuleId);
webgpu_resource!(WebGPUTexture, id::TextureId);
webgpu_resource!(WebGPUTextureView, id::TextureViewId);
