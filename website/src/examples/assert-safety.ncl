auth_token = 'Assert 'Absent,

PermitRootLogin =
  'Assert (fun { target } =>
    target != "yes"),
