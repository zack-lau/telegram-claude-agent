resource "aws_cloudwatch_log_group" "ecs" {
  name              = "/ecs/${local.name_prefix}"
  retention_in_days = 30
  kms_key_id        = aws_kms_key.logs.arn
}

resource "aws_cloudwatch_metric_alarm" "alb_5xx" {
  alarm_name          = "${local.name_prefix}-alb-5xx-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "HTTPCode_Target_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "ALB 5xx responses > 10 in 1 minute"
  treat_missing_data  = "breaching"
  alarm_actions       = var.alarm_sns_arn != "" ? [var.alarm_sns_arn] : []
  ok_actions          = var.alarm_sns_arn != "" ? [var.alarm_sns_arn] : []

  dimensions = {
    LoadBalancer = aws_lb.main.arn_suffix
  }
}

resource "aws_cloudwatch_metric_alarm" "ecs_running_tasks" {
  alarm_name          = "${local.name_prefix}-ecs-tasks-zero"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 3
  metric_name         = "RunningTaskCount"
  namespace           = "ECS/ContainerInsights"
  period              = 60
  statistic           = "Average"
  threshold           = 1
  alarm_description   = "ECS service has 0 running tasks for 3 consecutive minutes"
  treat_missing_data  = "breaching"
  alarm_actions       = var.alarm_sns_arn != "" ? [var.alarm_sns_arn] : []

  dimensions = {
    ClusterName = aws_ecs_cluster.main.name
    ServiceName = aws_ecs_service.api.name
  }
}

# Alert on oversized body spike — SizeRestrictions_BODY is overridden to count
# (not block) because Aegis clients POST encrypted payloads > 8KB. A spike above
# 100 requests/5min indicates potential abuse and warrants investigation.
resource "aws_cloudwatch_metric_alarm" "waf_oversize_body_spike" {
  alarm_name          = "${local.name_prefix}-waf-oversize-body-spike"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "CountedRequests"
  namespace           = "AWS/WAFV2"
  period              = 300
  statistic           = "Sum"
  threshold           = 100
  alarm_description   = "WAF SizeRestrictions_BODY counted > 100 requests in 5 min — potential abuse"
  treat_missing_data  = "notBreaching"
  alarm_actions       = var.alarm_sns_arn != "" ? [var.alarm_sns_arn] : []

  dimensions = {
    WebACL    = aws_wafv2_web_acl.main.name
    Rule      = "SizeRestrictions_BODY"
    RuleGroup = "AWSManagedRulesCommonRuleSet"
    Region    = var.aws_region
  }
}
