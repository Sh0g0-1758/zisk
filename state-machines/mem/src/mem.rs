use std::sync::Arc;

use crate::{
    InputDataSM, MemAlignInstance, MemAlignRomSM, MemAlignSM, MemCounters, MemModuleInstance,
    MemPlanner, MemSM, RomDataSM,
};
use data_bus::MEM_BUS_ID;
use p3_field::PrimeField64;
use pil_std_lib::Std;
use proofman_common::ProofCtx;
use sm_common::{table_instance, BusDeviceMetrics, ComponentBuilder, InstanceCtx, Plan, Planner};
use zisk_pil::{
    InputDataTrace, MemAlignRomTrace, MemAlignTrace, MemTrace, RomDataTrace, ZiskProofValues,
};

pub struct Mem<F: PrimeField64> {
    // Secondary State machines
    mem_sm: Arc<MemSM<F>>,
    mem_align_sm: Arc<MemAlignSM<F>>,
    mem_align_rom_sm: Arc<MemAlignRomSM>,
    input_data_sm: Arc<InputDataSM<F>>,
    rom_data_sm: Arc<RomDataSM<F>>,
}

impl<F: PrimeField64> Mem<F> {
    pub fn new(std: Arc<Std<F>>) -> Arc<Self> {
        let mem_align_rom_sm = MemAlignRomSM::new();
        let mem_align_sm = MemAlignSM::new(std.clone(), mem_align_rom_sm.clone());
        let mem_sm = MemSM::new(std.clone());
        let input_data_sm = InputDataSM::new(std.clone());
        let rom_data_sm = RomDataSM::new(std.clone());

        Arc::new(Self { mem_align_sm, mem_align_rom_sm, mem_sm, input_data_sm, rom_data_sm })
    }
}

impl<F: PrimeField64> ComponentBuilder<F> for Mem<F> {
    fn build_counter(&self) -> Box<dyn BusDeviceMetrics> {
        Box::new(MemCounters::new())
    }

    fn build_planner(&self) -> Box<dyn Planner> {
        Box::new(MemPlanner::new())
    }

    fn configure_instances(&self, pctx: &ProofCtx<F>, plannings: &[Plan]) {
        let enable_input_data =
            plannings.iter().any(|p| p.air_id == InputDataTrace::<usize>::AIR_ID);
        let mut proof_values = ZiskProofValues::from_vec_guard(pctx.get_proof_values());
        proof_values.enable_input_data = F::from_bool(enable_input_data);
    }

    /// Builds an instance of the Memory state machine.
    ///
    /// # Arguments
    /// * `ictx` - The context of the instance, containing the plan and its associated
    ///
    /// # Returns
    /// A boxed implementation of a Memory Instance.
    fn build_instance(&self, ictx: InstanceCtx) -> Box<dyn sm_common::Instance<F>> {
        match ictx.plan.air_id {
            MemTrace::<usize>::AIR_ID => {
                Box::new(MemModuleInstance::new(self.mem_sm.clone(), ictx, true))
            }
            RomDataTrace::<usize>::AIR_ID => {
                Box::new(MemModuleInstance::new(self.rom_data_sm.clone(), ictx, false))
            }
            InputDataTrace::<usize>::AIR_ID => {
                Box::new(MemModuleInstance::new(self.input_data_sm.clone(), ictx, false))
            }
            MemAlignTrace::<usize>::AIR_ID => {
                Box::new(MemAlignInstance::new(self.mem_align_sm.clone(), ictx))
            }
            MemAlignRomTrace::<usize>::AIR_ID => {
                table_instance!(MemAlignRomInstance, MemAlignRomSM, MemAlignRomTrace);
                Box::new(MemAlignRomInstance::new(self.mem_align_rom_sm.clone(), ictx, MEM_BUS_ID))
            }
            _ => panic!("Memory::get_instance() Unsupported air_id: {:?}", ictx.plan.air_id),
        }
    }
}
