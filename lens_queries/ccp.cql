library Retrieve
using FHIR version '4.0.0'
include FHIRHelpers version '4.0.0'

codesystem loinc: 'http://loinc.org'

context Patient


DKTK_STRAT_GENDER_STRATIFIER

DKTK_STRAT_PRIMARY_DIAGNOSIS_NO_SORT_STRATIFIER
DKTK_STRAT_AGE_CLASS_STRATIFIER

DKTK_STRAT_DECEASED_STRATIFIER

DKTK_STRAT_DIAGNOSIS_STRATIFIER

DKTK_REPLACE_SPECIMEN_STRATIFIERif InInitialPopulation then [Specimen] else {} as List<Specimen>
DKTK_STRAT_PROCEDURE_STRATIFIER

DKTK_STRAT_MEDICATION_STRATIFIER

  DKTK_REPLACE_HISTOLOGY_STRATIFIER
 if histo.code.coding.where(code = '59847-4').code.first() is null then 0 else 1
DKTK_STRAT_DEF_IN_INITIAL_POPULATION
