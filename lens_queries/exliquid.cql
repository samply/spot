library Retrieve
using FHIR version '4.0.0'
include FHIRHelpers version '4.0.0'
codesystem SampleMaterialType: 'https://fhir.bbmri.de/CodeSystem/SampleMaterialType'

codesystem loinc: 'http://loinc.org'

context Patient
EXLIQUID_CQL_DIAGNOSIS EXLIQUID_CQL_SPECIMEN EXLIQUID_STRAT_DEF_IN_INITIAL_POPULATION
