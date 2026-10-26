### Fixed
- The checker validates key types in all 29 documented btrees against reference-observed allowances, including hash whiteouts in xattrs and the common set and boundary-type controls.
- Wrong-type keys use the common `invalid_bkey` finding code, including previously recognized pairs, and each offender is reported once.
