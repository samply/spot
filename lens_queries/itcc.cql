library Retrieve
using FHIR version '4.0.0'
include FHIRHelpers version '4.0.0'
codesystem SampleMaterialType: 'https://fhir.bbmri.de/CodeSystem/SampleMaterialType'

codesystem loinc: 'http://loinc.org'

context Patient
DKTK_STRAT_GENDER_STRATIFIER
  DKTK_STRAT_GENETIC_VARIANT

  ITCC_STRAT_DIAGNOSIS_STRATIFIER
  ITCC_STRAT_AGE_CLASS_STRATIFIER
  DKTK_STRAT_DEF_IN_INITIAL_POPULATION
