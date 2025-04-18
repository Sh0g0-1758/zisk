use crate::{
    MemHelpers, MemInput, MemModule, MemModuleSegmentCheckPoint, MemPreviousSegment,
    STEP_MEMORY_MAX_DIFF,
};
use data_bus::{BusDevice, BusId, MemBusData, PayloadType, MEM_BUS_ID};
use p3_field::PrimeField;
use proofman_common::{AirInstance, ProofCtx, SetupCtx};
use proofman_util::{timer_start_debug, timer_stop_and_log_debug};
use sm_common::{BusDeviceWrapper, CheckPoint, Instance, InstanceCtx, InstanceType};
use std::ops::Add;
use std::sync::Arc;
use zisk_common::{ChunkId, SegmentId};

pub struct MemModuleInstance<F: PrimeField> {
    /// Instance context
    ictx: InstanceCtx,

    module: Arc<dyn MemModule<F>>,

    mem_check_point: MemModuleSegmentCheckPoint,
    min_addr: u32,
    #[allow(dead_code)]
    max_addr: u32,
    limited_step_distance: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct MemLastValue {
    pub segment_id: SegmentId,
    pub checkpoint_addr: u32,
    pub checkpoint_step: u64,
    pub value: u64,
    pub step: u64,
    pub addr: u32,
}

impl MemLastValue {
    pub fn new(segment_id: SegmentId, checkpoint_addr: u32, checkpoint_step: u64) -> Self {
        Self { segment_id, checkpoint_addr, checkpoint_step, value: 0, step: 0, addr: 0 }
    }
    pub fn update(&mut self, value: u64, addr_w: u32, step: u64) {
        if addr_w > self.checkpoint_addr {
            return;
        }
        #[allow(clippy::comparison_chain)]
        if addr_w > self.addr {
            if addr_w < self.checkpoint_addr || step <= self.checkpoint_step {
                self.set(value, addr_w, step);
            }
        } else if addr_w == self.addr && step > self.step && step <= self.checkpoint_step {
            self.set(value, addr_w, step);
        }
    }
    pub fn set(&mut self, value: u64, addr_w: u32, step: u64) {
        self.value = value;
        self.step = step;
        self.addr = addr_w;
    }
}

impl Add for MemLastValue {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if self.checkpoint_addr != 0 {
            assert_eq!(self.checkpoint_addr, other.checkpoint_addr);
            assert_eq!(self.checkpoint_step, other.checkpoint_step);
            assert_eq!(self.segment_id, other.segment_id);
        }
        if self.addr > other.addr || (self.addr == other.addr && self.step > other.step) {
            self
        } else {
            other
        }
    }
}

impl<F: PrimeField> MemModuleInstance<F> {
    pub fn new(
        module: Arc<dyn MemModule<F>>,
        ictx: InstanceCtx,
        limited_step_distance: bool,
    ) -> Self {
        let meta = ictx.plan.meta.as_ref().unwrap();
        let mem_check_point = meta.downcast_ref::<MemModuleSegmentCheckPoint>().unwrap().clone();

        let (min_addr, max_addr) = module.get_addr_range();
        Self {
            ictx,
            module: module.clone(),
            mem_check_point,
            min_addr,
            max_addr,
            limited_step_distance,
        }
    }

    fn prepare_inputs(&mut self, inputs: &mut [MemInput]) {
        // sort all instance inputs
        timer_start_debug!(MEM_SORT);
        inputs.sort_by_key(|input| (input.addr, input.step));
        timer_stop_and_log_debug!(MEM_SORT);
    }

    /// This method calculates intermediate accesses without adding inputs and trims
    /// the inputs while considering skipped rows for this instance.
    ///
    /// Additionally, it computes the necessary information for memory continuations.
    /// It returns the previous segment information.
    ///
    /// # Arguments
    /// * `inputs` - The inputs to be processed.
    /// * `mem_check_point` - The memory check point.
    /// * `prev_last_value` - The previous last value.
    ///
    /// # Returns
    /// The previous segment information.
    fn fit_inputs_and_get_prev_segment(
        &mut self,
        inputs: &mut Vec<MemInput>,
        mem_check_point: MemModuleSegmentCheckPoint,
        last_value: &mut MemLastValue,
    ) -> MemPreviousSegment {
        let mut last_step = mem_check_point.prev_step;
        #[cfg(feature = "debug_mem")]
        let initial = (inputs[0].addr, inputs[0].step, inputs.len());

        if mem_check_point.skip_rows > 1 {
            // rows to be skipped
            let check_point_skip_rows = mem_check_point.skip_rows - 1;
            let mut input_index = 0;

            // rows skipped at the moment
            let mut skipped_rows = 0;

            // at this point we need to skip at least one row, this
            // row value could be prev_segment.value(last), we update it
            last_value.set(
                inputs[input_index].value,
                inputs[input_index].addr,
                inputs[input_index].step,
            );

            loop {
                // we interested only in segment addr, but we need to check
                // if there are intermidate accesses

                while self.limited_step_distance
                    && inputs[input_index].addr == mem_check_point.prev_addr
                    && (inputs[input_index].step - last_step) > STEP_MEMORY_MAX_DIFF
                    && skipped_rows < check_point_skip_rows as usize
                {
                    // we skip an intermediate row
                    last_step += STEP_MEMORY_MAX_DIFF;
                    skipped_rows += 1;
                }
                if skipped_rows >= check_point_skip_rows as usize {
                    break;
                }

                assert_eq!(mem_check_point.prev_addr, inputs[input_index].addr);
                last_step = inputs[input_index].step;
                last_value.set(inputs[input_index].value, inputs[input_index].addr, last_step);
                input_index += 1;
                skipped_rows += 1;
            }
            inputs.drain(0..input_index);
        }

        #[cfg(feature = "debug_mem")]
        let original_inputs_len = inputs.len();

        inputs.truncate(mem_check_point.rows as usize);

        #[cfg(feature = "debug_mem")]
        println!(
            "[Mem:{}] #1 INPUT [0x{:X},{}] {} => [0x{:X},{}] {} => {} F [0x{:X},{},skip:{}]-[0x{:X},{}]",
            self.ictx.plan.segment_id.unwrap(),
            initial.0,
            initial.1,
            initial.2,
            inputs[0].addr,
            inputs[0].step,
            original_inputs_len,
            inputs.len(),
            self.mem_check_point.prev_addr,
            self.mem_check_point.prev_step,
            self.mem_check_point.skip_rows,
            self.mem_check_point.last_addr,
            self.mem_check_point.last_step,
        );
        MemPreviousSegment {
            addr: mem_check_point.prev_addr,
            step: last_step,
            value: last_value.value,
        }
    }
}

impl<F: PrimeField> Instance<F> for MemModuleInstance<F> {
    fn compute_witness(
        &mut self,
        _pctx: &ProofCtx<F>,
        _sctx: &SetupCtx<F>,
        collectors: Vec<(usize, Box<BusDeviceWrapper<PayloadType>>)>,
    ) -> Option<AirInstance<F>> {
        // Collect inputs from all collectors. At most, one of them has `prev_last_value` non zero,
        // we take this `prev_last_value`, which represents the last value of the previous segment.

        let mut last_value = MemLastValue::new(SegmentId::new(0), 0, 0);
        let inputs: Vec<_> = collectors
            .into_iter()
            .map(|(_, mut collector)| {
                let mem_module_collector =
                    collector.detach_device().as_any().downcast::<MemModuleCollector>().unwrap();

                last_value = last_value + mem_module_collector.last_value;
                mem_module_collector.inputs
            })
            .collect();
        let mut inputs = inputs.into_iter().flatten().collect::<Vec<_>>();

        if inputs.is_empty() {
            return None;
        }

        // This method sorts all inputs
        self.prepare_inputs(&mut inputs);

        // This method calculates intermediate accesses without adding inputs and trims
        // the inputs while considering skipped rows for this instance.
        // Additionally, it computes the necessary information for memory continuations.
        let prev_segment = self.fit_inputs_and_get_prev_segment(
            &mut inputs,
            self.mem_check_point.clone(),
            &mut last_value,
        );

        // Extract segment id from instance context
        let segment_id = self.ictx.plan.segment_id.unwrap();

        let is_last_segment = self.mem_check_point.is_last_segment;
        Some(self.module.compute_witness(&inputs, segment_id, is_last_segment, &prev_segment))
    }

    /// Builds an input collector for the instance.
    ///
    /// # Arguments
    /// * `chunk_id` - The chunk ID associated with the input collector.
    ///
    /// # Returns
    /// An `Option` containing the input collector for the instance.
    fn build_inputs_collector(
        &self,
        _chunk_id: ChunkId,
    ) -> Option<Box<dyn BusDevice<PayloadType>>> {
        Some(Box::new(MemModuleCollector::new(
            self.mem_check_point.clone(),
            self.min_addr,
            self.ictx.plan.segment_id.unwrap(),
        )))
    }

    fn check_point(&self) -> CheckPoint {
        self.ictx.plan.check_point.clone()
    }

    fn instance_type(&self) -> InstanceType {
        InstanceType::Instance
    }
}

pub struct MemModuleCollector {
    /// Binary Basic state machine
    mem_check_point: MemModuleSegmentCheckPoint,

    /// Collected inputs
    inputs: Vec<MemInput>,

    last_value: MemLastValue,

    min_addr: u32,
}

impl MemModuleCollector {
    pub fn new(
        mem_check_point: MemModuleSegmentCheckPoint,
        min_addr: u32,
        segment_id: SegmentId,
    ) -> Self {
        let prev_addr = mem_check_point.prev_addr;
        let prev_step = mem_check_point.prev_step;
        Self {
            inputs: Vec::new(),
            mem_check_point,
            last_value: MemLastValue::new(segment_id, prev_addr, prev_step),
            min_addr,
        }
    }

    /// Processes an unaligned memory access.
    ///
    /// Processes an unaligned memory access by computing all necessary aligned memory operations
    /// required to validate the unaligned access. The method determines the specific access case
    /// based on whether it involves a single or double memory access and calls the appropriate
    /// method to handle it.
    ///
    /// # Parameters
    /// - `data`: The data associated with the memory access.
    fn process_unaligned_data(&mut self, data: &[u64]) {
        let addr = MemBusData::get_addr(data);
        let addr_w = MemHelpers::get_addr_w(addr);
        let bytes = MemBusData::get_bytes(data);
        let is_write = MemHelpers::is_write(MemBusData::get_op(data));
        if MemHelpers::is_double(addr, bytes) {
            if is_write {
                self.process_unaligned_double_write(addr_w, bytes, data);
            } else {
                self.process_unaligned_double_read(addr_w, data);
            }
        } else if is_write {
            self.process_unaligned_single_write(addr_w, bytes, data);
        } else {
            self.process_unaligned_single_read(addr_w, data);
        }
    }

    /// Processes an unaligned single read operation.
    ///
    /// Handles an unaligned single read operation by computing all necessary aligned memory
    /// operations required to validate the unaligned access. Finally, it uses `filtered_inputs_push`
    /// to push only the necessary memory accesses into `inputs`.
    ///
    /// # Parameters
    /// - `addr_w`: The memory address (aligned to 8 bytes).
    /// - `data`: The data associated with the memory access.
    fn process_unaligned_single_read(&mut self, addr_w: u32, data: &[u64]) {
        let value = MemBusData::get_mem_values(data)[0];
        let step = MemBusData::get_step(data);
        self.filtered_inputs_push(addr_w, step, false, value);
    }

    /// Processes an unaligned single write operation.
    ///
    /// Handles an unaligned single write operation by computing all necessary aligned memory
    /// operations required to validate the unaligned access. Additionally, it calculates the
    /// write value for the given memory access. Finally, it uses `filtered_inputs_push` to
    /// push only the necessary memory accesses into `inputs`.
    ///
    /// # Parameters
    /// - `addr_w`: The memory address (aligned to 8 bytes).
    /// - `bytes`: The number of bytes to be written.
    /// - `data`: The data associated with the memory access.
    fn process_unaligned_single_write(&mut self, addr_w: u32, bytes: u8, data: &[u64]) {
        let read_values = MemBusData::get_mem_values(data);
        let write_values = MemHelpers::get_write_values(
            MemBusData::get_addr(data),
            bytes,
            MemBusData::get_value(data),
            read_values,
        );
        let step = MemBusData::get_step(data);
        self.filtered_inputs_push(addr_w, MemHelpers::get_read_step(step), false, read_values[0]);
        self.filtered_inputs_push(addr_w, MemHelpers::get_write_step(step), true, write_values[0]);
    }

    /// Processes an unaligned double read operation.
    ///
    /// Handles an unaligned double read operation by computing all necessary aligned memory
    /// operations required to validate the unaligned access. Finally, it uses `filtered_inputs_push`
    /// to push only the necessary memory accesses into `inputs`.
    ///
    /// # Parameters
    /// - `addr_w`: The memory address (aligned to 8 bytes).
    /// - `data`: The data associated with the memory access.
    fn process_unaligned_double_read(&mut self, addr_w: u32, data: &[u64]) {
        let read_values = MemBusData::get_mem_values(data);
        let step = MemBusData::get_step(data);
        self.filtered_inputs_push(addr_w, step, false, read_values[0]);
        self.filtered_inputs_push(addr_w + 1, step, false, read_values[1]);
    }

    /// Processes an unaligned double write operation.
    ///
    /// Handles an unaligned double write operation by computing all necessary aligned memory
    /// operations required to validate the unaligned access. Additionally, it calculates the
    /// write value for the given memory access. Finally, it uses `filtered_inputs_push` to
    /// push only the necessary memory accesses into `inputs`.
    ///
    /// # Parameters
    /// - `addr_w`: The memory address (aligned to 8 bytes).
    /// - `bytes`: The number of bytes to be written.
    /// - `data`: The data associated with the memory access.
    fn process_unaligned_double_write(&mut self, addr_w: u32, bytes: u8, data: &[u64]) {
        let read_values = MemBusData::get_mem_values(data);
        let write_values = MemHelpers::get_write_values(
            MemBusData::get_addr(data),
            bytes,
            MemBusData::get_value(data),
            read_values,
        );
        let step = MemBusData::get_step(data);
        let read_step = MemHelpers::get_read_step(step);
        let write_step = MemHelpers::get_write_step(step);

        // IMPORTANT: inputs must be ordered by step
        self.filtered_inputs_push(addr_w, read_step, false, read_values[0]);
        self.filtered_inputs_push(addr_w + 1, read_step, false, read_values[1]);

        self.filtered_inputs_push(addr_w, write_step, true, write_values[0]);
        self.filtered_inputs_push(addr_w + 1, write_step, true, write_values[1]);
    }

    /// Discards the given memory access if it is not part of the current segment.
    ///
    /// This function checks whether the given memory access (defined by `addr`, `step`, and `value`)
    /// should be discarded. If the access is not part of the current segment, the function returns `true`.
    ///
    /// # Parameters
    /// - `addr`: The memory address (8 bytes aligned).
    /// - `step`: The mem_step of the memory access.
    /// - `value`: The value to be read or written.
    ///
    /// # Returns
    /// `true` if the access should be discarded, `false` otherwise.
    fn discart_addr_step(&mut self, addr_w: u32, step: u64, value: u64) -> bool {
        if addr_w > self.mem_check_point.last_addr || addr_w < self.min_addr {
            return true;
        }

        self.last_value.update(value, addr_w, step);
        if addr_w < self.mem_check_point.prev_addr {
            return true;
        }

        // Edge case where only the cutting address and step are known, but the value at this point
        // is unknown. This value must be saved because it is needed for memory continuations,
        // it represents the last value of the previous segment.

        if addr_w == self.mem_check_point.prev_addr {
            match step.cmp(&self.mem_check_point.prev_step) {
                std::cmp::Ordering::Less => {
                    return true;
                }
                std::cmp::Ordering::Equal => {
                    return true;
                }
                std::cmp::Ordering::Greater => {}
            }
        }

        if addr_w == self.mem_check_point.last_addr && step > self.mem_check_point.last_step {
            return true;
        }

        false
    }

    /// Pushes the access only if this access must be managed with current instance.
    ///
    /// This function checks whether the given memory access (defined by `addr_w`, `step`, and `value`)
    /// should be discarded using `discard_addr_step()`. If it is not discarded,
    /// a new `MemInput` instance is created and pushed to `inputs`.
    ///
    /// # Parameters
    /// - `addr_w`: The memory address (8 bytes aligned).
    /// - `step`: The mem_step of the memory access.
    /// - `is_write`: Indicates whether the access is a write operation.
    /// - `value`: The value to be read or written.
    fn filtered_inputs_push(&mut self, addr_w: u32, step: u64, is_write: bool, value: u64) {
        if !self.discart_addr_step(addr_w, step, value) {
            self.inputs.push(MemInput::new(addr_w, is_write, step, value));
        }
    }
}

impl BusDevice<u64> for MemModuleCollector {
    fn process_data(&mut self, bus_id: &BusId, data: &[u64]) -> Option<Vec<(BusId, Vec<u64>)>> {
        debug_assert!(*bus_id == MEM_BUS_ID);

        // decoding information in bus

        let addr = MemBusData::get_addr(data);
        let step = MemBusData::get_step(data);
        let bytes = MemBusData::get_bytes(data);

        // If the access is unaligned (not aligned to 8 bytes or has a width different from 8 bytes).

        if !MemHelpers::is_aligned(addr, bytes) {
            self.process_unaligned_data(data);
            return None;
        }

        // Direct case when is aligned, calculated 8 bytes addres (addr_w) and check if is a
        // write or read.

        let addr_w = MemHelpers::get_addr_w(addr);
        let is_write = MemHelpers::is_write(MemBusData::get_op(data));
        if is_write {
            self.filtered_inputs_push(addr_w, step, true, MemBusData::get_value(data));
        } else {
            self.filtered_inputs_push(addr_w, step, false, MemBusData::get_mem_values(data)[0]);
        }

        None
    }

    fn bus_id(&self) -> Vec<BusId> {
        vec![MEM_BUS_ID]
    }

    /// Provides a dynamic reference for downcasting purposes.
    fn as_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}
