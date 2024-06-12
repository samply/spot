use serde::Serialize;


#[derive(Serialize)]
pub struct HealthOutput {
    pub summary: Verdict,
    pub beam: BeamStatus
}

#[derive(Serialize)]
pub enum Verdict {
    Healthy,
}

#[derive(Serialize)]
pub enum BeamStatus {
    Ok,
}