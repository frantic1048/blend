timeout =
  fun { target } =>
    if target == 'Absent then 30 else target,
